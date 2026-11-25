use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const END_MS: u32 = 600_000;
const PERIOD_MS: u32 = 20;
const ENABLE_AT: u32 = 4_000;
const MOVE_AT: u32 = 4_000;
const RESETS: [(u32, u32); 4] = [
    (0, 4_000),
    (5_500, 7_500),
    (30_000, 31_200),
    (598_000, 600_001),
];

struct Key {
    t: u32,
    pos: f64,
    speed: f64,
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

fn zone_of(pos: f64) -> i32 {
    if pos < 8_000.0 {
        -1
    } else if pos > 54_000.0 {
        1
    } else if (24_000.0..40_000.0).contains(&pos) {
        0
    } else {
        2
    }
}

struct Cue {
    keys: Vec<Key>,
    t: u32,
    pos: f64,
    speed: f64,
    last_rate: f64,
    zone: i32,
    zone_since: u32,
    heading: f64,
}

impl Cue {
    fn room(&self) -> u32 {
        END_MS.saturating_sub(self.t).saturating_sub(1)
    }

    fn push(&mut self, pos: f64, speed: f64, dur: u32) {
        let dur = dur.min(self.room());
        if dur == 0 {
            return;
        }
        self.t += dur;
        self.pos = pos.clamp(0.0, 65_535.0);
        self.speed = speed.clamp(0.0, 255.0);
        self.keys.push(Key {
            t: self.t,
            pos: self.pos,
            speed: self.speed,
        });
        let zone = zone_of(self.pos);
        if zone != self.zone {
            self.zone = zone;
            self.zone_since = self.t;
        }
    }

    fn near(&self, pos: f64) -> f64 {
        match self.zone {
            -1 => pos.clamp(80.0, 6_800.0),
            0 => pos.clamp(25_200.0, 38_800.0),
            1 => pos.clamp(55_200.0, 65_100.0),
            _ => pos.clamp(40.0, 65_300.0),
        }
    }

    fn go_for(&mut self, rng: &mut Rng, target: f64, rate: f64, max_ms: u32) {
        let target = target.clamp(0.0, 65_535.0);
        let dist = (target - self.pos).abs();
        if dist < 1.0 || self.room() < 200 || max_ms < 160 {
            return;
        }
        let rate = rate.clamp(30.0, 4_000.0);
        let natural = ((dist / rate) * 1000.0).round().max(160.0) as u32;
        let dur = natural.min(self.room()).min(max_ms);
        let dest = if dur + 30 < natural {
            let traveled = rate * f64::from(dur) / 1000.0;
            self.pos + (target - self.pos).signum() * traveled.min(dist)
        } else {
            target
        };
        self.sliced(rng, dest, dur, rate);
    }

    fn sliced(&mut self, rng: &mut Rng, dest: f64, dur: u32, rate: f64) {
        let mut left = dur.min(self.room());
        let mut prev_frame = f64::NAN;
        while left >= 220 && self.room() >= 140 {
            let slice = rng.range_u(140, 280).min(left).min(self.room());
            if slice < 140 {
                break;
            }
            let remain = dest - self.pos;
            if remain.abs() < 80.0 {
                break;
            }
            let step = {
                let fair = remain * f64::from(slice) / f64::from(left);
                let roll = rng.f64();
                let ease = if rate > 1_400.0 { 0.06 } else { 0.12 };
                if roll < ease {
                    fair * rng.range(0.25, 0.5)
                } else if rate > 1_400.0 {
                    fair * rng.range(0.82, 1.16)
                } else {
                    fair * rng.range(0.6, 1.35)
                }
            };
            let cap = 4_200.0 * f64::from(slice) / 1000.0;
            let mut step = step.clamp(-cap, cap);
            let mut per_frame = step / f64::from(slice) * 20.0;
            let alike =
                prev_frame.abs() > 1.0 && (per_frame - prev_frame).abs() < prev_frame.abs() * 0.18;
            if alike {
                let gain = if step.abs() > cap * 0.8 { 0.7 } else { 1.35 };
                step = (step * gain).clamp(-cap, cap);
                per_frame = step / f64::from(slice) * 20.0;
            }
            prev_frame = per_frame;
            self.push((self.pos + step).clamp(0.0, 65_535.0), self.speed, slice);
            left = left.saturating_sub(slice);
        }
        if left >= 80 && (dest - self.pos).abs() > 40.0 {
            self.push_capped(dest, left.min(160), rate);
        }
    }

    fn push_capped(&mut self, dest: f64, dur: u32, rate: f64) {
        let cap = rate.min(4_200.0) * f64::from(dur) / 1000.0;
        let delta = dest - self.pos;
        let dest = if delta.abs() > cap {
            self.pos + delta.signum() * cap
        } else {
            dest
        };
        self.push(dest, self.speed, dur);
    }

    fn rest(&mut self, dur: u32) {
        self.push(self.pos, self.speed, dur.max(200));
    }

    fn pick_rate(&mut self, rng: &mut Rng, low: f64, high: f64) -> f64 {
        let mut rate = rng.range(low, high);
        if (rate - self.last_rate).abs() < (high - low) * 0.2 {
            rate = if rate < (low + high) * 0.5 { high } else { low };
        }
        self.last_rate = rate;
        rate
    }

    fn place(&self, side: f64, pos: f64) -> f64 {
        if side < 0.0 {
            pos.clamp(150.0, 16_000.0)
        } else {
            pos.clamp(49_000.0, 64_800.0)
        }
    }

    fn live(&mut self, rng: &mut Rng, side: f64) {
        if self.room() < 400 {
            return;
        }
        if rng.f64() < 0.22 {
            self.heading = -self.heading;
        }
        let roll = rng.f64();
        if roll < 0.12 {
            self.rest(rng.range_u(400, 1_400));
        } else if roll < 0.3 {
            let dist = rng.range(120.0, 1_400.0);
            let dur = rng.range_u(1_200, 3_200);
            let target = self.place(side, self.pos + self.heading * dist);
            let rate = self.pick_rate(rng, 80.0, 350.0);
            self.go_for(rng, target, rate, dur);
        } else if roll < 0.68 {
            let dist = rng.range(1_200.0, 5_000.0);
            let dur = rng.range_u(1_200, 3_000);
            let target = self.place(side, self.pos + self.heading * dist);
            let rate = self.pick_rate(rng, 700.0, 1_800.0);
            self.go_for(rng, target, rate, dur);
        } else if roll < 0.84 {
            self.dart(rng, side);
        } else if roll < 0.93 {
            let dist = rng.range(400.0, 1_800.0);
            let dur = rng.range_u(600, 1_600);
            let target = self.place(side, self.pos - self.heading * dist);
            let rate = self.pick_rate(rng, 400.0, 1_100.0);
            self.go_for(rng, target, rate, dur);
            self.heading = -self.heading;
        } else {
            self.feed(rng);
        }
    }

    fn dart(&mut self, rng: &mut Rng, side: f64) {
        if self.room() < 700 {
            return;
        }
        let dir = if rng.f64() < 0.5 { -1.0 } else { 1.0 };
        self.heading = dir;
        let jump = dir * rng.range(2_500.0, 7_000.0);
        let snapped = self.place(side, self.pos + jump * rng.range(0.55, 0.9));
        let burst = self.pick_rate(rng, 2_200.0, 3_800.0);
        let burst_ms = rng.range_u(700, 1_600);
        self.go_for(rng, snapped, burst, burst_ms);
        let landed = self.place(side, self.pos + dir * rng.range(400.0, 1_800.0));
        let tail = self.pick_rate(rng, 800.0, 1_800.0);
        let tail_ms = rng.range_u(400, 900);
        self.go_for(rng, landed, tail, tail_ms);
    }

    fn cross(&mut self, rng: &mut Rng, side: f64) {
        let goal = if side > 0.0 {
            rng.range(52_000.0, 63_500.0)
        } else {
            rng.range(600.0, 12_000.0)
        };
        let mut legs = 0;
        while (self.pos - goal).abs() > 2_500.0 && self.room() > 900 && legs < 12 {
            legs += 1;
            let dir = (goal - self.pos).signum();
            let roll = rng.f64();
            if roll < 0.18 {
                self.rest(rng.range_u(300, 1_200));
            } else if roll < 0.3 {
                let back = (self.pos - dir * rng.range(150.0, 700.0)).clamp(40.0, 65_300.0);
                let rate = self.pick_rate(rng, 300.0, 800.0);
                let dur = rng.range_u(500, 1_400);
                self.go_for(rng, back, rate, dur);
            } else if roll < 0.48 {
                let jump = (self.pos + dir * rng.range(2_000.0, 5_500.0)).clamp(40.0, 65_300.0);
                let rate = self.pick_rate(rng, 1_800.0, 3_400.0);
                let dur = rng.range_u(700, 1_800);
                self.go_for(rng, jump, rate, dur);
            } else {
                let step = (self.pos + dir * rng.range(1_200.0, 3_600.0)).clamp(40.0, 65_300.0);
                let rate = self.pick_rate(rng, 800.0, 1_800.0);
                let dur = rng.range_u(800, 2_200);
                self.go_for(rng, step, rate, dur);
            }
        }
        if (self.pos - goal).abs() > 2_500.0 && self.room() > 600 {
            let rate = self.pick_rate(rng, 1_400.0, 2_600.0);
            self.go_for(rng, goal, rate, 4_000);
        }
        self.heading = if rng.f64() < 0.5 { -1.0 } else { 1.0 };
    }

    fn feed(&mut self, rng: &mut Rng) {
        if self.room() < 8_000 {
            return;
        }
        let used = self.t.saturating_sub(self.zone_since);
        let meal = rng
            .range_u(2_500, 6_000)
            .min(36_000u32.saturating_sub(used))
            .min(self.room().saturating_sub(1_500));
        if meal < 2_500 {
            return;
        }
        let until = self.t.saturating_add(meal);
        let anchor = self.pos;
        while self.t < until && self.room() > 500 {
            let step_ms = until.saturating_sub(self.t).clamp(200, 2_600);
            let roll = rng.f64();
            if roll < 0.48 {
                self.rest(rng.range_u(200, 800).min(step_ms));
            } else if roll < 0.78 {
                let target = self.near(anchor + rng.range(-900.0, 700.0));
                let rate = self.pick_rate(rng, 120.0, 400.0);
                self.go_for(rng, target, rate, step_ms);
            } else {
                let target = self.near(self.pos + rng.range(-1_800.0, 600.0));
                let rate = self.pick_rate(rng, 200.0, 600.0);
                self.go_for(rng, target, rate, step_ms.min(1_000));
            }
        }
    }
}

fn main() {
    let first = choreography(0xB11D_0101, -1.0);
    let second = choreography(0x0B11_7A11, 1.0);
    let left = analyze(&first, &second, 0);
    let right = analyze(&first, &second, 10);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("blind01.NCOD");
    println!(
        "{}  {} frames  {:.3}s",
        path.display(),
        left.frames,
        f64::from(END_MS) / 1000.0,
    );
    for second_mark in (0..=600).step_by(30) {
        let slots = slots_at(&first, &second, second_mark * 1000);
        let a = u16::from(slots[0]) << 8 | u16::from(slots[1]);
        let b = u16::from(slots[10]) << 8 | u16::from(slots[11]);
        println!(
            "t={:6.1}s  a={a:5} ch3={:3}  b={b:5} ch13={:3}",
            f64::from(second_mark),
            slots[2],
            slots[12],
        );
    }
    let mut apart = 0.0f64;
    let mut samples = 0.0f64;
    let mut mark = ENABLE_AT;
    while mark <= END_MS {
        if !resetting(mark) {
            let (a, _) = sample(&first, mark);
            let (b, _) = sample(&second, mark);
            apart += (a - b).abs();
            samples += 1.0;
        }
        mark += 200;
    }
    assert!(
        apart / samples > 8_000.0,
        "the two blinds are living the same life"
    );
    for (name, stats) in [("ch1", &left), ("ch11", &right)] {
        assert_eq!(stats.max_ch3, 255, "{name} speed left 255");
        assert_eq!(stats.min_ch3_after, 255, "{name} speed dropped");
        assert_eq!(stats.max_ch4, 0, "{name} limit channel moved");
        assert!(
            stats.max_rate > 1_800.0 && stats.max_rate < 4_800.0,
            "{name} fulgurance peak {:.0}/s",
            stats.max_rate
        );
        assert!(stats.cautious_s > 12.0, "{name} never really slow");
        assert!(stats.sharp_s > 8.0, "{name} never really quick");
        assert!(
            stats.low_s > 40.0 && stats.high_s > stats.low_s * 1.4,
            "{name} high should dominate, low {:.0}s high {:.0}s",
            stats.low_s,
            stats.high_s
        );
        assert!(
            stats.reversals > 35,
            "{name} motion is still too homogeneous"
        );
        assert!(
            stats.turns > 22,
            "{name} direction is still a shuttle, only {} turns",
            stats.turns
        );
        assert!(
            stats.max_glide_ms < 700,
            "{name} a glide stays linear for {} ms",
            stats.max_glide_ms
        );
        println!(
            "{name}  pos {:.0}->{:.0}  peak {:.0}/s  turns {}  low {:.0}s  high {:.0}s  cautious {:.0}s  sharp {:.0}s",
            stats.min_pos,
            stats.max_pos,
            stats.max_rate,
            stats.turns,
            stats.low_s,
            stats.high_s,
            stats.cautious_s,
            stats.sharp_s
        );
    }
    write_show(&path, &first, &second).expect("write blind01.NCOD");
}

fn choreography(seed: u64, mut side: f64) -> Vec<Key> {
    let mut rng = Rng(seed);
    let start = if side > 0.0 {
        rng.range(51_000.0, 62_000.0)
    } else {
        0.0
    };
    let mut cue = Cue {
        keys: vec![
            Key {
                t: 0,
                pos: 0.0,
                speed: 0.0,
            },
            Key {
                t: MOVE_AT,
                pos: start,
                speed: 0.0,
            },
        ],
        t: MOVE_AT,
        pos: start,
        speed: 255.0,
        last_rate: 0.0,
        zone: zone_of(start),
        zone_since: ENABLE_AT,
        heading: if rng.f64() < 0.5 { -1.0 } else { 1.0 },
    };
    while cue.room() > 16_000 {
        let stay = if side > 0.0 {
            rng.range_u(55_000, 130_000)
        } else {
            rng.range_u(22_000, 48_000)
        };
        let stay = stay.min(cue.room() - 8_000);
        let until = cue.t.saturating_add(stay);
        while cue.t < until && cue.room() > 1_200 {
            cue.live(&mut rng, side);
        }
        if cue.room() > 10_000 {
            side = -side;
            cue.cross(&mut rng, side);
        }
    }
    if side < 0.0 && cue.room() > 6_000 {
        cue.cross(&mut rng, 1.0);
        side = 1.0;
    }
    while cue.room() > 1_500 {
        cue.live(&mut rng, side);
    }
    if cue.t < END_MS {
        cue.rest(END_MS - cue.t);
    }
    cue.keys
}

struct Stats {
    frames: u32,
    min_pos: f64,
    max_pos: f64,
    max_ch3: u8,
    max_ch4: u8,
    max_rate: f64,
    min_ch3_after: u8,
    reversals: u32,
    max_glide_ms: u32,
    turns: u32,
    low_s: f64,
    high_s: f64,
    cautious_s: f64,
    sharp_s: f64,
}

fn analyze(first: &[Key], second: &[Key], base: usize) -> Stats {
    let mut frames = 0u32;
    let mut min_pos = f64::MAX;
    let mut max_pos = 0.0f64;
    let mut max_ch3 = 0u8;
    let mut max_ch4 = 0u8;
    let mut max_rate = 0.0f64;
    let mut min_ch3_after = 255u8;
    let mut reversals = 0u32;
    let mut prev_delta = 0.0f64;
    let mut glide = 0u32;
    let mut max_glide_ms = 0u32;
    let mut low_s = 0.0f64;
    let mut high_s = 0.0f64;
    let mut cautious_s = 0.0f64;
    let mut sharp_s = 0.0f64;
    let mut prev_step = 0.0f64;
    let mut prev_pos = 0.0f64;
    let mut prev_t = 0u32;
    let mut t = 0u32;
    loop {
        let slots = slots_at(first, second, t);
        if resetting(t) {
            assert!(slots.iter().all(|byte| *byte == 0), "reset broken at {t}");
        }
        assert_eq!(slots[3], 0, "channel 4 must stay at 0");
        assert!(slots[4..10].iter().all(|byte| *byte == 0));
        assert_eq!(slots[13], 0, "channel 14 must stay at 0");
        assert!(slots[14..].iter().all(|byte| *byte == 0));
        let pos = f64::from(u16::from(slots[base]) << 8 | u16::from(slots[base + 1]));
        if t >= ENABLE_AT && !resetting(t) {
            min_pos = min_pos.min(pos);
            max_pos = max_pos.max(pos);
            assert_eq!(slots[base + 2], 255, "speed left 255 at {t}");
            max_ch3 = max_ch3.max(slots[base + 2]);
            min_ch3_after = min_ch3_after.min(slots[base + 2]);
            max_ch4 = max_ch4.max(slots[base + 3]);
            if pos < 18_000.0 {
                low_s += f64::from(PERIOD_MS) / 1000.0;
            } else if pos > 46_000.0 {
                high_s += f64::from(PERIOD_MS) / 1000.0;
            }
        }
        if t > 0 && !resetting(t) && !resetting(prev_t) {
            let delta = pos - prev_pos;
            let rate = delta.abs() / (f64::from(t - prev_t) / 1000.0);
            max_rate = max_rate.max(rate);
            if t >= ENABLE_AT && (40.0..900.0).contains(&rate) {
                cautious_s += f64::from(PERIOD_MS) / 1000.0;
            }
            if t >= ENABLE_AT && rate > 1_600.0 {
                sharp_s += f64::from(PERIOD_MS) / 1000.0;
            }
            if delta.abs() > 1.5 && prev_delta.abs() > 1.5 && delta.signum() != prev_delta.signum()
            {
                reversals += 1;
            }
            if delta.abs() > 1.5 {
                prev_delta = delta;
            }
            let scale = delta.abs().max(prev_step.abs()).max(1.0);
            if delta.abs() > 3.0 && (delta - prev_step).abs() < scale * 0.08 {
                glide += PERIOD_MS;
                max_glide_ms = max_glide_ms.max(glide);
            } else {
                glide = if delta.abs() > 8.0 { PERIOD_MS } else { 0 };
            }
            prev_step = delta;
        }
        frames += 1;
        prev_pos = pos;
        prev_t = t;
        if t >= END_MS {
            break;
        }
        t = (t + PERIOD_MS).min(END_MS);
    }
    let mut turns = 0u32;
    let mut prev_sign = 0.0;
    let mut mark = ENABLE_AT;
    while mark + 6_000 <= END_MS {
        let own = if base == 0 { first } else { second };
        let (start, _) = sample(own, mark);
        let (end, _) = sample(own, mark + 6_000);
        let drift = end - start;
        if drift.abs() > 180.0 {
            let sign = drift.signum();
            if prev_sign != 0.0 && sign != prev_sign {
                turns += 1;
            }
            prev_sign = sign;
        }
        mark += 6_000;
    }
    Stats {
        frames,
        min_pos,
        max_pos,
        max_ch3,
        max_ch4,
        max_rate,
        min_ch3_after,
        reversals,
        max_glide_ms,
        turns,
        low_s,
        high_s,
        cautious_s,
        sharp_s,
    }
}

fn sample(keys: &[Key], t: u32) -> (f64, f64) {
    if t <= keys[0].t {
        return (keys[0].pos, keys[0].speed);
    }
    let last = keys.last().unwrap();
    if t >= last.t {
        return (last.pos, last.speed);
    }
    let index = keys.partition_point(|key| key.t <= t) - 1;
    let start = &keys[index];
    let end = &keys[index + 1];
    let span = f64::from(end.t - start.t);
    let u = f64::from(t - start.t) / span;
    (
        start.pos + (end.pos - start.pos) * u,
        start.speed + (end.speed - start.speed) * u,
    )
}

fn resetting(t: u32) -> bool {
    RESETS
        .iter()
        .any(|(start, end)| (*start..*end).contains(&t))
}

fn write_blind(slots: &mut [u8; 512], at: usize, keys: &[Key], t: u32) {
    let (pos, _) = sample(keys, t);
    let pos = pos.round().clamp(0.0, 65_535.0) as u16;
    slots[at] = (pos >> 8) as u8;
    slots[at + 1] = (pos & 0xff) as u8;
    slots[at + 2] = 255;
}

fn slots_at(first: &[Key], second: &[Key], t: u32) -> [u8; 512] {
    let mut slots = [0u8; 512];
    if resetting(t) {
        return slots;
    }
    write_blind(&mut slots, 0, first, t);
    write_blind(&mut slots, 10, second, t);
    slots
}

fn write_show(path: &PathBuf, first: &[Key], second: &[Key]) -> std::io::Result<()> {
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
        let slots = slots_at(first, second, t);
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
