use std::env;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const END_MS: u32 = 600_000;
const PERIOD_MS: u32 = 20;
const ALT: usize = 400;
const SPEED: usize = 406;
const ACCEL: usize = 408;
const BULB: usize = 300;
const FULL: u16 = u16::MAX;
const PERCH: f64 = 54_000.0;
const UPPER: f64 = 43_690.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Program {
    Discovery,
    Mesocosme,
    Landscape,
    Steady,
    Phare,
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

struct AltKey {
    t: u32,
    pos: f64,
}

struct Alt {
    keys: Vec<AltKey>,
    t: u32,
    pos: f64,
    last_rate: f64,
    heading: f64,
    low_at: u32,
    mid_at: u32,
    high_at: u32,
    program: Program,
}

impl Alt {
    fn room(&self) -> u32 {
        END_MS.saturating_sub(self.t)
    }

    fn here(&self) -> u8 {
        if self.pos < 12_000.0 {
            0
        } else if self.pos > 48_000.0 {
            2
        } else {
            1
        }
    }

    fn push_pos(&mut self, pos: f64, dur: u32) {
        let dur = dur.min(self.room());
        if dur == 0 {
            return;
        }
        self.t += dur;
        self.pos = pos.clamp(0.0, 65_535.0);
        if self.pos < 12_000.0 {
            self.low_at = self.t;
        } else if self.pos > 48_000.0 {
            self.high_at = self.t;
        } else {
            self.mid_at = self.t;
        }
        self.keys.push(AltKey {
            t: self.t,
            pos: self.pos,
        });
    }

    fn rest(&mut self, dur: u32) {
        self.push_pos(self.pos, dur);
    }

    fn rate(&mut self, rng: &mut Rng, low: f64, high: f64) -> f64 {
        let mut rate = rng.range(low, high);
        if (rate - self.last_rate).abs() < (high - low) * 0.22 {
            rate = if rate < (low + high) * 0.5 { high } else { low };
        }
        self.last_rate = rate;
        rate
    }

    fn aim(&mut self, dist: f64) -> f64 {
        if self.heading < 0.0 && self.pos < 1_500.0 {
            self.heading = 1.0;
        } else if self.heading > 0.0 && self.pos > 64_000.0 {
            self.heading = -1.0;
        }
        (self.pos + self.heading * dist).clamp(0.0, 65_535.0)
    }

    fn go(&mut self, rng: &mut Rng, target: f64, rate: f64, max_ms: u32) {
        let target = target.clamp(0.0, 65_535.0);
        let dist = (target - self.pos).abs();
        if dist < 1.0 || self.room() < 200 || max_ms < 160 {
            return;
        }
        let rate = rate.clamp(30.0, self.speed_cap() - 200.0);
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
            let fair = remain * f64::from(slice) / f64::from(left);
            let roll = rng.f64();
            let ease = if rate > 1_400.0 { 0.06 } else { 0.12 };
            let step = if roll < ease {
                fair * rng.range(0.25, 0.5)
            } else if rate > 1_400.0 {
                fair * rng.range(0.82, 1.16)
            } else {
                fair * rng.range(0.6, 1.35)
            };
            let cap = self.speed_cap() * f64::from(slice) / 1000.0;
            let mut step = step.clamp(-cap, cap);
            let mut per_frame = step / f64::from(slice) * 20.0;
            let alike =
                prev_frame.abs() > 1.0 && (per_frame - prev_frame).abs() < prev_frame.abs() * 0.18;
            if alike {
                let kick = if step.abs() > cap * 0.8 { 0.7 } else { 1.35 };
                step = (step * kick).clamp(-cap, cap);
                per_frame = step / f64::from(slice) * 20.0;
            }
            prev_frame = per_frame;
            self.push_pos((self.pos + step).clamp(0.0, 65_535.0), slice);
            left = left.saturating_sub(slice);
        }
        if left >= 80 && (dest - self.pos).abs() > 40.0 {
            self.push_capped(dest, left.min(160), rate);
        }
    }

    fn push_capped(&mut self, dest: f64, dur: u32, rate: f64) {
        let cap = rate.min(self.speed_cap()) * f64::from(dur) / 1000.0;
        let delta = dest - self.pos;
        let dest = if delta.abs() > cap {
            self.pos + delta.signum() * cap
        } else {
            dest
        };
        self.push_pos(dest, dur);
    }

    fn dart(&mut self, rng: &mut Rng) {
        if self.room() < 800 {
            return;
        }
        let (dist, burst) = if self.program == Program::Mesocosme {
            (
                rng.range(10_000.0, 24_000.0),
                self.rate(rng, 4_400.0, 6_800.0),
            )
        } else {
            (
                rng.range(7_000.0, 18_000.0),
                self.rate(rng, 2_200.0, 3_900.0),
            )
        };
        let dest = self.aim(dist);
        let burst_ms = rng.range_u(500, 1_400);
        self.go(rng, dest, burst, burst_ms);
        let settle = rng.range(300.0, 1_400.0);
        let tail = self.rate(rng, 500.0, 1_200.0);
        let landed = self.aim(settle);
        let tail_ms = rng.range_u(350, 900);
        self.go(rng, landed, tail, tail_ms);
    }

    fn relocate(&mut self, rng: &mut Rng, target: f64) {
        let target = target.clamp(0.0, 65_535.0);
        let mut legs = 0u32;
        while (self.pos - target).abs() > 2_000.0 && self.room() > 800 && legs < 7 {
            legs += 1;
            let dir = (target - self.pos).signum();
            self.heading = dir;
            let roll = rng.f64();
            if roll < 0.14 {
                let pause = rng.range_u(200, 800);
                self.rest(pause);
            } else if roll < 0.24 {
                let back = (self.pos - dir * rng.range(150.0, 700.0)).clamp(0.0, 65_535.0);
                let cruise = self.rate(rng, 250.0, 700.0);
                let dur = rng.range_u(400, 1_000);
                self.go(rng, back, cruise, dur);
            } else if roll < 0.48 {
                let jump = (self.pos + dir * rng.range(3_000.0, 8_000.0)).clamp(0.0, 65_535.0);
                let cruise = self.rate(rng, 1_600.0, 3_200.0);
                let dur = rng.range_u(500, 1_400);
                self.go(rng, jump, cruise, dur);
            } else {
                let step = (self.pos + dir * rng.range(1_000.0, 3_400.0)).clamp(0.0, 65_535.0);
                let cruise = self.rate(rng, 500.0, 1_500.0);
                let dur = rng.range_u(700, 1_800);
                self.go(rng, step, cruise, dur);
            }
        }
        if (self.pos - target).abs() > 2_000.0 && self.room() > 600 {
            let cruise = self.rate(rng, 900.0, 2_000.0);
            self.go(rng, target, cruise, 4_000);
        }
        if Self::band_of(self.pos) == Self::band_of(target) {
            self.dwell(rng);
        }
    }

    fn dwell(&mut self, rng: &mut Rng) {
        let span = if self.program == Program::Mesocosme {
            rng.range_u(1_400, 3_400)
        } else {
            rng.range_u(8_000, 16_000)
        };
        let until = self
            .t
            .saturating_add(span)
            .min(END_MS.saturating_sub(2_000));
        let anchor = self.pos;
        while self.t < until && self.room() > 400 {
            let before = self.t;
            if rng.f64() < 0.58 {
                let pause = rng.range_u(400, 1_800);
                self.rest(pause);
            } else {
                let target = match self.here() {
                    0 => (anchor + rng.range(-700.0, 1_100.0)).clamp(250.0, 9_000.0),
                    2 => (anchor + rng.range(-1_100.0, 700.0)).clamp(52_000.0, 64_600.0),
                    _ => (anchor + rng.range(-1_600.0, 1_600.0)).clamp(16_000.0, 44_000.0),
                };
                let cruise = self.rate(rng, 80.0, 340.0);
                let dur = rng.range_u(500, 1_600);
                self.go(rng, target, cruise, dur);
            }
            if self.t == before {
                self.rest(240);
            }
        }
    }

    fn band_of(pos: f64) -> u8 {
        if pos < 12_000.0 {
            0
        } else if pos > 48_000.0 {
            2
        } else {
            1
        }
    }

    fn band_target(rng: &mut Rng, band: u8) -> f64 {
        match band {
            0 => rng.range(300.0, 8_000.0),
            1 => rng.range(24_000.0, 40_000.0),
            _ => rng.range(52_000.0, 64_200.0),
        }
    }

    fn speed_cap(&self) -> f64 {
        if self.program == Program::Mesocosme {
            7_400.0
        } else {
            4_200.0
        }
    }

    fn starved(&self) -> Option<u8> {
        let (gate, limit, room) = if self.program == Program::Mesocosme {
            (8_000, 16_000, 3_500)
        } else {
            (24_000, 36_000, 6_000)
        };
        if self.t < gate || self.room() < room {
            return None;
        }
        let mut order = [
            (0u8, self.t.saturating_sub(self.low_at)),
            (1, self.t.saturating_sub(self.mid_at)),
            (2, self.t.saturating_sub(self.high_at)),
        ];
        order.sort_by_key(|item| std::cmp::Reverse(item.1));
        let here = self.here();
        order
            .into_iter()
            .find(|(band, age)| *band != here && *age >= limit)
            .map(|(band, _)| band)
    }

    fn high_aim(&mut self, dist: f64) -> f64 {
        let floor = 45_500.0;
        let ceil = 64_700.0;
        if self.heading < 0.0 && self.pos < floor + 1_500.0 {
            self.heading = 1.0;
        } else if self.heading > 0.0 && self.pos > ceil - 1_200.0 {
            self.heading = -1.0;
        }
        (self.pos + self.heading * dist).clamp(floor, ceil)
    }

    fn hunt(&mut self, rng: &mut Rng) {
        if self.room() < 300 {
            self.rest(self.room());
            return;
        }
        let before = self.t;
        let roll = rng.f64();
        if roll < 0.18 {
            let pause = rng.range_u(160, 900);
            self.rest(pause);
        } else if roll < 0.50 {
            if rng.f64() < 0.45 {
                self.heading = -self.heading;
            }
            let dist = rng.range(60.0, 1_100.0);
            let dest = self.high_aim(dist);
            let cruise = self.rate(rng, 40.0, 240.0);
            let dur = rng.range_u(600, 2_200);
            self.go(rng, dest, cruise, dur);
        } else if roll < 0.78 {
            if rng.f64() < 0.4 {
                self.heading = -self.heading;
            }
            let dist = rng.range(900.0, 4_200.0);
            let dest = self.high_aim(dist);
            let cruise = self.rate(rng, 280.0, 1_100.0);
            let dur = rng.range_u(500, 1_800);
            self.go(rng, dest, cruise, dur);
        } else if roll < 0.93 {
            let dist = rng.range(2_800.0, 7_500.0);
            let dest = self.high_aim(dist);
            let cruise = self.rate(rng, 1_500.0, 3_200.0);
            let dur = rng.range_u(320, 900);
            self.go(rng, dest, cruise, dur);
        } else {
            let dip = rng.range(36_000.0, 42_000.0);
            let cruise = self.rate(rng, 400.0, 1_200.0);
            let dur = rng.range_u(600, 1_500);
            self.go(rng, dip, cruise, dur);
        }
        if self.t == before {
            self.rest(200);
        }
    }

    fn beacon_aim(&mut self, dist: f64) -> f64 {
        let floor = 52_000.0;
        let ceil = 63_500.0;
        if self.heading < 0.0 && self.pos < floor + 800.0 {
            self.heading = 1.0;
        } else if self.heading > 0.0 && self.pos > ceil - 800.0 {
            self.heading = -1.0;
        }
        (self.pos + self.heading * dist).clamp(floor, ceil)
    }

    fn beacon(&mut self, rng: &mut Rng) {
        if self.room() < 400 {
            self.rest(self.room());
            return;
        }
        let before = self.t;
        if rng.f64() < 0.7 {
            let pause = rng.range_u(2_500, 9_000);
            self.rest(pause);
        } else {
            if rng.f64() < 0.5 {
                self.heading = -self.heading;
            }
            let dist = rng.range(250.0, 1_600.0);
            let dest = self.beacon_aim(dist);
            let cruise = self.rate(rng, 30.0, 140.0);
            let dur = rng.range_u(2_500, 7_000);
            self.go(rng, dest, cruise, dur);
        }
        if self.t == before {
            self.rest(400);
        }
    }

    fn live(&mut self, rng: &mut Rng) {
        if self.program == Program::Landscape {
            self.hunt(rng);
            return;
        }
        if self.program == Program::Phare {
            self.beacon(rng);
            return;
        }
        if self.room() < 400 {
            self.rest(self.room());
            return;
        }
        let meso = self.program == Program::Mesocosme;
        let before = self.t;
        if let Some(band) = self.starved() {
            let target = Self::band_target(rng, band);
            self.relocate(rng, target);
        } else {
            let roll = rng.f64();
            let rest_at = if meso { 0.06 } else { 0.24 };
            let nudge_at = if meso { 0.20 } else { 0.48 };
            let travel_at = if meso { 0.55 } else { 0.78 };
            let dart_at = if meso { 0.84 } else { 0.90 };
            if roll < rest_at {
                let pause = if meso {
                    rng.range_u(120, 460)
                } else {
                    rng.range_u(500, 2_800)
                };
                self.rest(pause);
            } else if roll < nudge_at {
                if rng.f64() < 0.35 {
                    self.heading = -self.heading;
                }
                let dist = if meso {
                    rng.range(350.0, 2_200.0)
                } else {
                    rng.range(180.0, 1_600.0)
                };
                let cruise = if meso {
                    self.rate(rng, 280.0, 1_000.0)
                } else {
                    self.rate(rng, 70.0, 320.0)
                };
                let dest = self.aim(dist);
                let dur = if meso {
                    rng.range_u(280, 900)
                } else {
                    rng.range_u(900, 2_800)
                };
                self.go(rng, dest, cruise, dur);
            } else if roll < travel_at {
                if rng.f64() < 0.3 {
                    self.heading = -self.heading;
                }
                let dist = if meso {
                    rng.range(4_500.0, 15_000.0)
                } else {
                    rng.range(2_000.0, 8_000.0)
                };
                let cruise = if meso {
                    self.rate(rng, 1_800.0, 4_400.0)
                } else {
                    self.rate(rng, 450.0, 1_400.0)
                };
                let dest = self.aim(dist);
                let dur = if meso {
                    rng.range_u(400, 1_400)
                } else {
                    rng.range_u(1_000, 3_200)
                };
                self.go(rng, dest, cruise, dur);
            } else if roll < dart_at {
                if rng.f64() < 0.5 {
                    self.heading = -self.heading;
                }
                self.dart(rng);
            } else {
                let band = rng.range_u(0, 3) as u8;
                let target = Self::band_target(rng, band);
                self.relocate(rng, target);
            }
        }
        if self.t == before {
            self.rest(300);
        }
    }
}

#[derive(Clone, Copy)]
struct Look {
    phase: f64,
    width: f64,
    gain: f64,
    echo: f64,
}

struct GlowKey {
    t: u32,
    look: Look,
}

struct Glow {
    keys: Vec<GlowKey>,
    t: u32,
    look: Look,
    heading: f64,
    last_rate: f64,
    program: Program,
}

impl Glow {
    fn room(&self) -> u32 {
        END_MS.saturating_sub(self.t)
    }

    fn commit(&mut self, dur: u32) -> u32 {
        let dur = dur.min(self.room());
        if dur == 0 {
            return 0;
        }
        self.t += dur;
        self.look.gain = self.look.gain.clamp(0.0, 1.0);
        self.look.echo = self.look.echo.clamp(0.0, 1.0);
        self.look.width = self.look.width.clamp(0.2, 24.0);
        self.keys.push(GlowKey {
            t: self.t,
            look: self.look,
        });
        dur
    }

    fn hold(&mut self, dur: u32) {
        self.commit(dur);
    }

    fn pick(&mut self, rng: &mut Rng, low: f64, high: f64) -> f64 {
        let mut rate = rng.range(low, high);
        if (rate - self.last_rate).abs() < (high - low) * 0.2 {
            rate = if rate < (low + high) * 0.5 { high } else { low };
        }
        self.last_rate = rate;
        rate
    }

    fn glide(&mut self, rng: &mut Rng, look: Look, dur: u32) {
        let dur = dur.min(self.room());
        if dur < 80 {
            return;
        }
        let from = self.look;
        let mut left = dur;
        let mut fast = false;
        while left > 0 && self.room() > 0 {
            let closing = left < 200;
            let slice = if closing {
                left.min(self.room())
            } else {
                rng.range_u(90, 180).min(left - 1).min(self.room())
            };
            if slice == 0 {
                break;
            }
            if closing {
                self.look = look;
            } else {
                let remain = look.phase - self.look.phase;
                let mut step = 0.0;
                if remain.abs() >= 0.015 {
                    let fair = remain * f64::from(slice) / f64::from(left);
                    let kick = if fast {
                        rng.range(1.25, 1.65)
                    } else {
                        rng.range(0.38, 0.68)
                    };
                    fast = !fast;
                    step = fair * kick;
                    let cap_rate = if self.program == Program::Mesocosme {
                        22.0
                    } else {
                        14.0
                    };
                    let cap = remain.abs().min(cap_rate * f64::from(slice) / 1000.0);
                    step = if remain.is_sign_negative() {
                        step.clamp(-cap, 0.0)
                    } else {
                        step.clamp(0.0, cap)
                    };
                }
                let done = f64::from(dur - (left - slice)) / f64::from(dur);
                self.look.phase += step;
                self.look.width = from.width + (look.width - from.width) * done;
                self.look.gain = from.gain + (look.gain - from.gain) * done;
                self.look.echo = from.echo + (look.echo - from.echo) * done;
            }
            let advanced = self.commit(slice);
            if advanced == 0 {
                break;
            }
            left = left.saturating_sub(advanced);
        }
    }

    fn step_chase(&mut self, rng: &mut Rng) {
        let meso = self.program == Program::Mesocosme;
        let steps = if meso {
            rng.range_u(6, 14)
        } else {
            rng.range_u(4, 9)
        };
        for _ in 0..steps {
            if self.room() < 300 {
                return;
            }
            let dest = self.look.phase + self.heading * FRAC_PI_2;
            let width = if meso {
                rng.range(1.14, 1.32)
            } else {
                rng.range(1.22, 1.48)
            };
            let hop = if meso {
                rng.range_u(90, 170)
            } else {
                rng.range_u(160, 420)
            };
            self.glide(
                rng,
                Look {
                    phase: dest,
                    width,
                    gain: 1.0,
                    echo: 0.0,
                },
                hop,
            );
            let pause = if meso {
                rng.range_u(40, 130)
            } else {
                rng.range_u(160, 700)
            }
            .min(self.room());
            self.hold(pause);
        }
    }

    fn spin(&mut self, rng: &mut Rng, fast: bool) {
        let meso = self.program == Program::Mesocosme;
        let rate = if meso && fast {
            self.pick(rng, 6.5, 14.0)
        } else if meso {
            self.pick(rng, 2.4, 5.8)
        } else if fast {
            self.pick(rng, 2.6, 6.8)
        } else {
            self.pick(rng, 0.55, 2.1)
        };
        let turns = if meso {
            rng.range(0.7, 2.0)
        } else if fast {
            rng.range(0.45, 1.5)
        } else {
            rng.range(0.35, 1.25)
        };
        let longest = if meso { 2_600.0 } else { 6_500.0 };
        let dur = ((turns * TAU / rate) * 1000.0)
            .round()
            .clamp(280.0, longest) as u32;
        let dur = dur.min(self.room());
        let dest = self.look.phase + self.heading * rate * f64::from(dur) / 1000.0;
        let width = if fast {
            rng.range(1.22, 1.5)
        } else {
            rng.range(1.3, 1.85)
        };
        self.glide(
            rng,
            Look {
                phase: dest,
                width,
                gain: 1.0,
                echo: 0.0,
            },
            dur,
        );
    }

    fn drift(&mut self, rng: &mut Rng) {
        let rate = self.pick(rng, 0.15, 0.5);
        let dur = rng.range_u(2_400, 7_000).min(self.room());
        let dest = self.look.phase + self.heading * rate * f64::from(dur) / 1000.0;
        let width = rng.range(1.7, 2.2);
        self.glide(
            rng,
            Look {
                phase: dest,
                width,
                gain: 1.0,
                echo: 0.0,
            },
            dur,
        );
    }

    fn stutter(&mut self, rng: &mut Rng) {
        let times = rng.range_u(2, 5);
        for _ in 0..times {
            if self.room() < 500 {
                return;
            }
            let forth = self.look.phase + self.heading * FRAC_PI_2 * rng.range(0.72, 1.12);
            let forth_ms = rng.range_u(160, 360);
            self.glide(
                rng,
                Look {
                    phase: forth,
                    width: 1.28,
                    gain: 1.0,
                    echo: 0.0,
                },
                forth_ms,
            );
            let back = self.look.phase - self.heading * FRAC_PI_2 * rng.range(0.16, 0.46);
            let back_ms = rng.range_u(180, 420);
            self.glide(
                rng,
                Look {
                    phase: back,
                    width: 1.34,
                    gain: 1.0,
                    echo: 0.0,
                },
                back_ms,
            );
            let pause = rng.range_u(90, 280).min(self.room());
            self.hold(pause);
        }
    }

    fn whip(&mut self, rng: &mut Rng) {
        if self.room() < 500 {
            return;
        }
        let rate = if self.program == Program::Mesocosme {
            self.pick(rng, 12.0, 20.0)
        } else {
            self.pick(rng, 7.5, 12.0)
        };
        let turns = rng.range(0.45, 1.2);
        let dur = ((turns * TAU / rate) * 1000.0)
            .round()
            .clamp(280.0, 1_100.0) as u32;
        let dur = dur.min(self.room());
        let dest = self.look.phase + self.heading * rate * f64::from(dur) / 1000.0;
        self.glide(
            rng,
            Look {
                phase: dest,
                width: 1.26,
                gain: 1.0,
                echo: 0.0,
            },
            dur,
        );
        let pause = if self.program == Program::Mesocosme {
            rng.range_u(50, 160)
        } else {
            rng.range_u(160, 640)
        }
        .min(self.room());
        self.hold(pause);
    }

    fn pair(&mut self, rng: &mut Rng) {
        if self.room() < 1_400 {
            return;
        }
        let pole = (self.look.phase / FRAC_PI_2).round() * FRAC_PI_2;
        let align = rng.range_u(320, 640);
        self.glide(
            rng,
            Look {
                phase: pole,
                width: 1.5,
                gain: 1.0,
                echo: 1.0,
            },
            align,
        );
        let pause = rng.range_u(300, 900).min(self.room());
        self.hold(pause);
        let turns = rng.range(0.5, 1.6);
        let rate = self.pick(rng, 0.45, 1.7);
        let dur = ((turns * TAU / rate) * 1000.0)
            .round()
            .clamp(900.0, 4_500.0) as u32;
        let dur = dur.min(self.room());
        let dest = self.look.phase + self.heading * rate * f64::from(dur) / 1000.0;
        let width = rng.range(1.42, 1.65);
        self.glide(
            rng,
            Look {
                phase: dest,
                width,
                gain: 1.0,
                echo: 1.0,
            },
            dur,
        );
        let release = self.look.phase + self.heading * 0.45;
        let release_ms = rng.range_u(360, 800).min(self.room());
        self.glide(
            rng,
            Look {
                phase: release,
                width: 1.4,
                gain: 1.0,
                echo: 0.0,
            },
            release_ms,
        );
    }

    fn bloom(&mut self, rng: &mut Rng) {
        if self.room() < 1_000 {
            return;
        }
        let spread = rng.range(0.2, 1.1);
        let open_ms = rng.range_u(320, 800);
        let dest = self.look.phase + self.heading * spread;
        self.glide(
            rng,
            Look {
                phase: dest,
                width: 8.0,
                gain: 1.0,
                echo: 0.7,
            },
            open_ms,
        );
        let pause = rng.range_u(280, 1_200).min(self.room().saturating_sub(500));
        self.hold(pause);
        let close = rng.range(0.15, 0.7);
        let close_ms = rng.range_u(360, 800).min(self.room());
        let dest = self.look.phase + self.heading * close;
        self.glide(
            rng,
            Look {
                phase: dest,
                width: 1.38,
                gain: 1.0,
                echo: 0.0,
            },
            close_ms,
        );
    }

    fn black(&mut self, rng: &mut Rng) {
        if self.room() < 1_100 {
            return;
        }
        let fall = rng.range_u(140, 280);
        let phase = self.look.phase;
        let width = self.look.width;
        self.glide(
            rng,
            Look {
                phase,
                width,
                gain: 0.0,
                echo: 0.0,
            },
            fall,
        );
        let gap = rng.range_u(500, 1_200).min(self.room().saturating_sub(350));
        self.hold(gap);
        let quarters = f64::from(rng.range_u(1, 4));
        let dest = self.look.phase + self.heading * FRAC_PI_2 * quarters;
        let hidden = 90.min(self.room());
        self.glide(
            rng,
            Look {
                phase: dest,
                width: 1.3,
                gain: 0.0,
                echo: 0.0,
            },
            hidden,
        );
        let rise = rng.range_u(160, 380).min(self.room());
        let phase = self.look.phase;
        self.glide(
            rng,
            Look {
                phase,
                width: 1.32,
                gain: 1.0,
                echo: 0.0,
            },
            rise,
        );
    }

    fn play_meso(&mut self, rng: &mut Rng) {
        if self.room() < 160 {
            self.hold(self.room());
            return;
        }
        let before = self.t;
        if rng.f64() < 0.42 {
            self.heading = -self.heading;
        }
        let roll = rng.f64();
        if roll < 0.22 {
            self.step_chase(rng);
        } else if roll < 0.50 {
            self.spin(rng, true);
        } else if roll < 0.68 {
            self.spin(rng, false);
        } else if roll < 0.80 {
            self.whip(rng);
        } else if roll < 0.90 {
            self.stutter(rng);
        } else if roll < 0.96 {
            self.pair(rng);
        } else {
            self.bloom(rng);
        }
        if self.t == before {
            self.hold(100.min(self.room()));
        }
    }

    fn play(&mut self, rng: &mut Rng) {
        if self.program == Program::Mesocosme {
            self.play_meso(rng);
            return;
        }
        if self.room() < 200 {
            self.hold(self.room());
            return;
        }
        let before = self.t;
        if rng.f64() < 0.24 {
            self.heading = -self.heading;
        }
        let roll = rng.f64();
        if roll < 0.30 {
            self.step_chase(rng);
        } else if roll < 0.48 {
            self.spin(rng, false);
        } else if roll < 0.60 {
            self.spin(rng, true);
        } else if roll < 0.70 {
            self.drift(rng);
        } else if roll < 0.79 {
            self.stutter(rng);
        } else if roll < 0.86 {
            self.whip(rng);
        } else if roll < 0.92 {
            self.pair(rng);
        } else if roll < 0.94 {
            let pause = rng.range_u(300, 1_200);
            self.hold(pause);
        } else if roll < 0.97 {
            self.bloom(rng);
        } else {
            self.black(rng);
        }
        if self.t == before {
            self.hold(180.min(self.room()));
        }
    }

    fn face(&self, offset: i32) -> f64 {
        let index = (self.look.phase / FRAC_PI_2).round();
        (index + self.heading * f64::from(offset)) * FRAC_PI_2
    }

    fn beam(&mut self, rng: &mut Rng, dest: f64, dur: u32, width: f64) {
        let dur = dur.min(self.room());
        let look = Look {
            phase: dest,
            width,
            gain: 1.0,
            echo: 0.0,
        };
        if dur < 80 {
            self.look = look;
            self.commit(dur);
            return;
        }
        self.glide(rng, look, dur);
    }

    fn toward(&mut self, rng: &mut Rng, offset: i32, reach: f64, dur: u32, width: f64) {
        let here = self.face(0);
        let there = self.face(offset);
        let dest = here + (there - here) * reach;
        self.beam(rng, dest, dur, width);
    }
}

fn lobe(phase: f64, pole: f64, width: f64) -> f64 {
    let width = width.max(0.2);
    let mut delta = (phase - pole).rem_euclid(TAU);
    if delta > PI {
        delta = TAU - delta;
    }
    if delta >= width {
        return 0.0;
    }
    let x = delta / width;
    0.5 * (1.0 + (PI * x).cos())
}

fn shade(look: Look) -> [u16; 4] {
    if look.width >= 20.0 {
        let level = (look.gain.clamp(0.0, 1.0) * f64::from(FULL))
            .round()
            .clamp(0.0, f64::from(FULL)) as u16;
        return [level; 4];
    }
    let mut levels = [0u16; 4];
    let gain = look.gain.clamp(0.0, 1.0);
    let echo = look.echo.clamp(0.0, 1.0);
    for (index, level) in levels.iter_mut().enumerate() {
        let pole = index as f64 * FRAC_PI_2;
        let mix = (lobe(look.phase, pole, look.width)
            + echo * lobe(look.phase + PI, pole, look.width))
        .clamp(0.0, 1.0);
        let shaped = mix.powf(1.35);
        *level = (gain * shaped * f64::from(FULL))
            .round()
            .clamp(0.0, f64::from(FULL)) as u16;
    }
    levels
}

fn main() {
    render(
        Program::Discovery,
        "discovery.NCOD",
        0xA170_0001,
        0x6E11_0004,
    );
    render(
        Program::Mesocosme,
        "mesocosme.NCOD",
        0x5E50_0001,
        0x5E50_00B1,
    );
    render(
        Program::Landscape,
        "landscape.NCOD",
        0x1A4D_0001,
        0x1A4D_00C1,
    );
    render(Program::Steady, "steady.NCOD", 0x57EA_0001, 0x57EA_0001);
    render(Program::Phare, "phare.NCOD", 0x0FA4_E001, 0x0FA4_E002);
}

fn render(program: Program, name: &str, alt_seed: u64, glow_seed: u64) {
    let altitude = altitude_show(alt_seed, program);
    let glow = glow_show(glow_seed, program);
    let stats = analyze(&altitude, &glow);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    println!(
        "{name}  alt {:.0}->{:.0}  peak {:.0}/s  rev {}  low {:.0}s  mid {:.0}s  high {:.0}s  upper {:.0}s",
        stats.min_pos,
        stats.max_pos,
        stats.max_rate,
        stats.reversals,
        stats.low_s,
        stats.mid_s,
        stats.high_s,
        stats.upper_s,
    );
    println!(
        "         sphere {:.1} turns  fwd {}  back {}  bulbs {:.0}/{:.0}/{:.0}/{:.0}  candle {}..{}  flips {}",
        stats.turns,
        stats.forward,
        stats.backward,
        stats.won[0],
        stats.won[1],
        stats.won[2],
        stats.won[3],
        stats.bulb0_min,
        stats.bulb0_max,
        stats.candle_flips,
    );
    enforce(program, &stats);
    write_show(&path, &altitude, &glow).unwrap_or_else(|_| panic!("write {name}"));
    confirm(&path, program);
}

struct Stats {
    min_pos: f64,
    max_pos: f64,
    max_rate: f64,
    reversals: u32,
    low_s: f64,
    mid_s: f64,
    high_s: f64,
    visits: [u32; 3],
    turns: f64,
    light_reversals: u32,
    forward: u32,
    backward: u32,
    dominant_s: f64,
    bloom_s: f64,
    dark_s: f64,
    pair_s: f64,
    won: [f64; 4],
    max_glide_ms: u32,
    max_phase_glide_ms: u32,
    upper_s: f64,
    bulb0_min: u16,
    bulb0_max: u16,
    other_max: u16,
    candle_flips: u32,
    uneven: u32,
}

fn analyze(altitude: &[AltKey], glow: &[GlowKey]) -> Stats {
    let mut min_pos = f64::MAX;
    let mut max_pos = 0.0f64;
    let mut max_rate = 0.0f64;
    let mut reversals = 0u32;
    let mut prev_delta = 0.0f64;
    let mut glide = 0u32;
    let mut max_glide_ms = 0u32;
    let mut prev_step = 0.0f64;
    let mut low_s = 0.0f64;
    let mut mid_s = 0.0f64;
    let mut high_s = 0.0f64;
    let mut visits = [0u32; 3];
    let mut prev_band = 9u8;
    let mut turns = 0.0f64;
    let mut light_reversals = 0u32;
    let mut prev_phi = 0.0f64;
    let mut phase_glide = 0u32;
    let mut max_phase_glide_ms = 0u32;
    let mut prev_dphi = 0.0f64;
    let mut forward = 0u32;
    let mut backward = 0u32;
    let mut winner: Option<usize> = None;
    let mut dominant_s = 0.0f64;
    let mut bloom_s = 0.0f64;
    let mut dark_s = 0.0f64;
    let mut pair_s = 0.0f64;
    let mut won = [0.0f64; 4];
    let mut upper_s = 0.0f64;
    let mut bulb0_min = u16::MAX;
    let mut bulb0_max = 0u16;
    let mut other_max = 0u16;
    let mut candle_flips = 0u32;
    let mut prev_bulb_delta = 0.0f64;
    let mut prev_bulb = 0.0f64;
    let mut uneven = 0u32;
    let mut prev_pos = 0.0f64;
    let mut prev_phase = 0.0f64;
    let mut prev_t = 0u32;
    let mut prev_live = false;
    let mut t = 0u32;
    loop {
        let slots = slots_at(altitude, glow, t);
        {
            assert_eq!(
                u16::from(slots[SPEED]) << 8 | u16::from(slots[SPEED + 1]),
                FULL
            );
            assert_eq!(
                u16::from(slots[ACCEL]) << 8 | u16::from(slots[ACCEL + 1]),
                FULL
            );
            assert!(slots[402..406].iter().all(|byte| *byte == 0), "gap at {t}");
            for (index, byte) in slots.iter().enumerate() {
                if !live_slot(index) {
                    assert_eq!(*byte, 0, "channel {} leaked at {t}", index + 1);
                }
            }
            let sent = u16::from(slots[ALT]) << 8 | u16::from(slots[ALT + 1]);
            let pos = f64::from(FULL - sent);
            min_pos = min_pos.min(pos);
            max_pos = max_pos.max(pos);
            let band = if pos < 12_000.0 {
                low_s += f64::from(PERIOD_MS) / 1000.0;
                0
            } else if pos > 48_000.0 {
                high_s += f64::from(PERIOD_MS) / 1000.0;
                2
            } else {
                mid_s += f64::from(PERIOD_MS) / 1000.0;
                1
            };
            if band != prev_band {
                visits[band as usize] += 1;
                prev_band = band;
            }
            if pos > UPPER {
                upper_s += f64::from(PERIOD_MS) / 1000.0;
            }
            let levels = bulb_levels(&slots);
            bulb0_min = bulb0_min.min(levels[0]);
            bulb0_max = bulb0_max.max(levels[0]);
            other_max = other_max.max(levels[1].max(levels[2]).max(levels[3]));
            if levels[0] != levels[1] || levels[1] != levels[2] || levels[2] != levels[3] {
                uneven += 1;
            }
            let (best_i, best, second) = rank(&levels);
            let dt = f64::from(PERIOD_MS) / 1000.0;
            if best < 1_500 {
                dark_s += dt;
            }
            if levels.iter().all(|level| *level > 20_000) {
                bloom_s += dt;
            }
            if axis(&levels) {
                pair_s += dt;
            }
            let dominant = u32::from(best) > 28_000 && u32::from(best) * 2 > u32::from(second) * 3;
            if dominant {
                dominant_s += dt;
                won[best_i] += dt;
                if let Some(prev) = winner {
                    if prev != best_i {
                        let step = (best_i + 4 - prev) % 4;
                        if step == 1 {
                            forward += 1;
                        } else if step == 3 {
                            backward += 1;
                        }
                    }
                }
                winner = Some(best_i);
            }
            if prev_live && t - prev_t == PERIOD_MS {
                let delta = pos - prev_pos;
                let rate = delta.abs() / (f64::from(PERIOD_MS) / 1000.0);
                max_rate = max_rate.max(rate);
                if delta.abs() > 2.0
                    && prev_delta.abs() > 2.0
                    && delta.signum() != prev_delta.signum()
                {
                    reversals += 1;
                }
                if delta.abs() > 2.0 {
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
                let dphi = prev_phase_delta(glow, t, prev_phase);
                turns += dphi.abs() / TAU;
                if dphi.abs() > 0.004
                    && prev_phi.abs() > 0.004
                    && dphi.signum() != prev_phi.signum()
                {
                    light_reversals += 1;
                }
                if dphi.abs() > 0.004 {
                    prev_phi = dphi;
                }
                let phi_scale = dphi.abs().max(prev_dphi.abs()).max(1e-4);
                if dphi.abs() > 0.004 && (dphi - prev_dphi).abs() < phi_scale * 0.08 {
                    phase_glide += PERIOD_MS;
                    max_phase_glide_ms = max_phase_glide_ms.max(phase_glide);
                } else {
                    phase_glide = if dphi.abs() > 0.01 { PERIOD_MS } else { 0 };
                }
                prev_dphi = dphi;
                let bulb_delta = f64::from(levels[0]) - prev_bulb;
                if bulb_delta.abs() > 400.0
                    && prev_bulb_delta.abs() > 400.0
                    && bulb_delta.signum() != prev_bulb_delta.signum()
                {
                    candle_flips += 1;
                }
                if bulb_delta.abs() > 400.0 {
                    prev_bulb_delta = bulb_delta;
                }
                prev_bulb = f64::from(levels[0]);
            }
            prev_pos = pos;
            prev_phase = sample_glow(glow, t).phase;
        }
        prev_live = true;
        prev_t = t;
        if t >= END_MS {
            break;
        }
        t = (t + PERIOD_MS).min(END_MS);
    }
    Stats {
        min_pos,
        max_pos,
        max_rate,
        reversals,
        low_s,
        mid_s,
        high_s,
        visits,
        turns,
        light_reversals,
        forward,
        backward,
        dominant_s,
        bloom_s,
        dark_s,
        pair_s,
        won,
        max_glide_ms,
        max_phase_glide_ms,
        upper_s,
        bulb0_min,
        bulb0_max,
        other_max,
        candle_flips,
        uneven,
    }
}

fn enforce(program: Program, stats: &Stats) {
    match program {
        Program::Discovery => enforce_discovery(stats),
        Program::Mesocosme => enforce_mesocosme(stats),
        Program::Landscape => enforce_landscape(stats),
        Program::Steady => enforce_steady(stats),
        Program::Phare => enforce_phare(stats),
    }
}

fn enforce_discovery(stats: &Stats) {
    assert!(stats.min_pos < 1_500.0, "never sat on the floor");
    assert!(stats.max_pos > 60_000.0, "never reached the top");
    assert!(
        stats.low_s > 30.0 && stats.high_s > 30.0 && stats.mid_s > 30.0,
        "a band is missing"
    );
    assert!(
        stats.visits[0] >= 3 && stats.visits[1] >= 3 && stats.visits[2] >= 3,
        "bands visited once"
    );
    assert!(stats.reversals > 18, "altitude is a one-way ride");
    assert!(
        stats.max_rate > 1_800.0 && stats.max_rate < 5_000.0,
        "altitude peak {}",
        stats.max_rate
    );
    assert!(
        stats.max_glide_ms < 750,
        "altitude glide {}ms",
        stats.max_glide_ms
    );
    assert!(
        stats.turns > 45.0,
        "the sphere barely turns ({})",
        stats.turns
    );
    assert!(stats.light_reversals > 15, "the chase never reverses");
    assert!(
        stats.forward > 12 && stats.backward > 8,
        "chase {} / {}",
        stats.forward,
        stats.backward
    );
    assert!(
        stats.won.iter().all(|seconds| *seconds > 30.0),
        "a bulb is left out"
    );
    assert!(
        stats.dominant_s > 100.0,
        "the point inside the sphere is washed out"
    );
    assert!(
        stats.bloom_s > 5.0 && stats.bloom_s < 70.0,
        "bloom {}",
        stats.bloom_s
    );
    assert!(
        stats.dark_s > 2.0 && stats.dark_s < 40.0,
        "dark {}",
        stats.dark_s
    );
    assert!(stats.pair_s > 8.0, "opposite axis never reads");
    assert!(
        stats.max_phase_glide_ms < 500,
        "phase glide {}ms",
        stats.max_phase_glide_ms
    );
}

fn enforce_mesocosme(stats: &Stats) {
    assert!(stats.min_pos < 1_500.0, "mesocosme never sat on the floor");
    assert!(stats.max_pos > 58_000.0, "mesocosme never reached the top");
    assert!(
        stats.low_s > 15.0 && stats.high_s > 15.0 && stats.mid_s > 15.0,
        "mesocosme band missing"
    );
    assert!(
        stats.max_rate > 5_000.0 && stats.max_rate < 8_000.0,
        "mesocosme peak {}",
        stats.max_rate
    );
    assert!(stats.reversals > 24, "mesocosme altitude is tame");
    assert!(
        stats.max_glide_ms < 750,
        "mesocosme glide {}",
        stats.max_glide_ms
    );
    assert!(stats.turns > 240.0, "mesocosme sphere {}", stats.turns);
    assert!(stats.light_reversals > 25, "mesocosme chase never reverses");
    assert!(
        stats.forward > 20 && stats.backward > 16,
        "mesocosme chase {} / {}",
        stats.forward,
        stats.backward
    );
    assert!(
        stats.won.iter().all(|seconds| *seconds > 20.0),
        "mesocosme bulb left out"
    );
    assert!(stats.dominant_s > 70.0, "mesocosme point washed out");
    assert!(
        stats.max_phase_glide_ms < 400,
        "mesocosme phase glide {}",
        stats.max_phase_glide_ms
    );
}

fn enforce_landscape(stats: &Stats) {
    assert!(
        stats.upper_s > 470.0,
        "landscape left the top {}",
        stats.upper_s
    );
    assert!(
        stats.min_pos > 34_000.0,
        "landscape dropped to {}",
        stats.min_pos
    );
    assert!(stats.max_pos > 60_000.0, "landscape never searched the top");
    assert!(stats.low_s < 1.0, "landscape visited the floor");
    assert!(stats.reversals > 30, "landscape motion is not an animal");
    assert!(
        stats.max_glide_ms < 800,
        "landscape glide {}",
        stats.max_glide_ms
    );
    assert_eq!(stats.uneven, 0, "landscape bulbs do not move together");
    assert_eq!(stats.other_max, stats.bulb0_max, "landscape bulbs split");
    assert!(stats.bulb0_max > 50_000, "candle never flares");
    assert!(stats.bulb0_min < 25_000, "candle never dips");
    assert!(
        stats.candle_flips > 80,
        "candle is a dimmer {}",
        stats.candle_flips
    );
}

fn enforce_phare(stats: &Stats) {
    assert!(stats.min_pos > 51_000.0, "phare left the top");
    assert!(stats.upper_s > 590.0, "phare upper {}", stats.upper_s);
    assert!(stats.low_s < 1.0, "phare reached the floor");
    assert!(
        stats.max_pos - stats.min_pos > 600.0,
        "phare altitude is frozen"
    );
    assert!(
        stats.max_rate > 20.0 && stats.max_rate < 400.0,
        "phare rate {}",
        stats.max_rate
    );
    assert!(
        stats.turns > 35.0 && stats.turns < 140.0,
        "phare turns {}",
        stats.turns
    );
    assert!(stats.forward > 80, "phare steps {}", stats.forward);
    assert!(
        stats.backward > 8 && stats.forward > stats.backward * 3,
        "phare mischief {} / {}",
        stats.forward,
        stats.backward
    );
    assert!(
        stats.won.iter().all(|seconds| *seconds > 60.0),
        "a face of the lighthouse is missing"
    );
    assert!(stats.dominant_s > 400.0, "the beam is not a single bulb");
    assert!(stats.pair_s < 2.0, "phare shows an axis");
    assert!(stats.dark_s < 2.0, "phare goes dark");
}

fn enforce_steady(stats: &Stats) {
    assert!((stats.min_pos - PERCH).abs() < 1.0, "steady moved");
    assert!((stats.max_pos - PERCH).abs() < 1.0, "steady moved");
    assert!(stats.upper_s > 590.0, "steady left the upper third");
    assert!(
        stats.max_rate < 1.0,
        "steady altitude rate {}",
        stats.max_rate
    );
    assert!(stats.turns < 0.05, "steady sphere turned");
    assert_eq!(stats.uneven, 0, "steady bulbs differ");
    assert_eq!(stats.bulb0_min, stats.bulb0_max, "steady lamp flickers");
    assert!(stats.bulb0_min == FULL, "steady lamp is not fully on");
}

fn prev_phase_delta(glow: &[GlowKey], t: u32, prev_phase: f64) -> f64 {
    sample_glow(glow, t).phase - prev_phase
}

fn rank(levels: &[u16; 4]) -> (usize, u16, u16) {
    let mut best_i = 0usize;
    let mut best = 0u16;
    let mut second = 0u16;
    for (index, level) in levels.iter().enumerate() {
        if *level >= best {
            second = best;
            best = *level;
            best_i = index;
        } else if *level > second {
            second = *level;
        }
    }
    (best_i, best, second)
}

fn axis(levels: &[u16; 4]) -> bool {
    let opposite = |a: usize, b: usize, c: usize, d: usize| {
        levels[a] > 25_000 && levels[b] > 25_000 && levels[c] < 10_000 && levels[d] < 10_000
    };
    opposite(0, 2, 1, 3) || opposite(1, 3, 0, 2)
}

fn bulb_levels(slots: &[u8; 512]) -> [u16; 4] {
    let mut levels = [0u16; 4];
    for (index, level) in levels.iter_mut().enumerate() {
        let at = BULB + index * 2;
        *level = u16::from(slots[at]) << 8 | u16::from(slots[at + 1]);
    }
    levels
}

fn live_slot(index: usize) -> bool {
    (300..308).contains(&index) || (400..402).contains(&index) || (406..410).contains(&index)
}

fn altitude_show(seed: u64, program: Program) -> Vec<AltKey> {
    let mut rng = Rng(seed);
    let start = match program {
        Program::Landscape => rng.range(52_000.0, 60_000.0),
        Program::Phare => rng.range(54_000.0, 60_000.0),
        Program::Steady => PERCH,
        _ => 0.0,
    };
    let heading = if program == Program::Landscape && rng.f64() < 0.5 {
        -1.0
    } else {
        1.0
    };
    let mut alt = Alt {
        keys: vec![AltKey { t: 0, pos: start }],
        t: 0,
        pos: start,
        last_rate: 0.0,
        heading,
        low_at: 0,
        mid_at: 0,
        high_at: 0,
        program,
    };
    if program == Program::Steady {
        alt.rest(END_MS);
        return alt.keys;
    }
    while alt.room() > 400 {
        alt.live(&mut rng);
    }
    if alt.t < END_MS {
        alt.rest(END_MS - alt.t);
    }
    alt.keys
}

fn glow_show(seed: u64, program: Program) -> Vec<GlowKey> {
    match program {
        Program::Landscape => candle_show(seed),
        Program::Phare => beacon_show(seed),
        Program::Steady => {
            let look = Look {
                phase: 0.0,
                width: 24.0,
                gain: 1.0,
                echo: 0.0,
            };
            vec![GlowKey { t: 0, look }, GlowKey { t: END_MS, look }]
        }
        _ => sphere_show(seed, program),
    }
}

fn sphere_show(seed: u64, program: Program) -> Vec<GlowKey> {
    let mut rng = Rng(seed);
    let pole = f64::from(rng.range_u(0, 4)) * FRAC_PI_2;
    let heading = if rng.f64() < 0.5 { -1.0 } else { 1.0 };
    let look = Look {
        phase: pole,
        width: 1.36,
        gain: 1.0,
        echo: 0.0,
    };
    let mut glow = Glow {
        keys: vec![GlowKey { t: 0, look }],
        t: 0,
        look,
        heading,
        last_rate: 0.0,
        program,
    };
    while glow.room() > 200 {
        glow.play(&mut rng);
    }
    if glow.t < END_MS {
        glow.hold(END_MS - glow.t);
    }
    glow.keys
}

fn beacon_show(seed: u64) -> Vec<GlowKey> {
    let mut rng = Rng(seed);
    let look = Look {
        phase: 0.0,
        width: 1.28,
        gain: 1.0,
        echo: 0.0,
    };
    let mut glow = Glow {
        keys: vec![GlowKey { t: 0, look }],
        t: 0,
        look,
        heading: 1.0,
        last_rate: 0.0,
        program: Program::Phare,
    };
    while glow.room() > 700 {
        let before = glow.t;
        let roll = rng.f64();
        if roll < 0.40 {
            let sweep = rng.range_u(180, 700);
            let width = rng.range(1.2, 1.4);
            glow.toward(&mut rng, 1, 1.0, sweep, width);
            let pause = rng.range_u(280, 2_200).min(glow.room());
            glow.hold(pause);
        } else if roll < 0.58 {
            let sweep = rng.range_u(350, 800);
            glow.toward(&mut rng, 1, 1.0, sweep, 1.16);
            let pause = rng.range_u(2_400, 5_200).min(glow.room());
            glow.hold(pause);
        } else if roll < 0.74 {
            let sweep = rng.range_u(90, 180);
            glow.toward(&mut rng, 1, 1.0, sweep, 1.24);
            let pause = rng.range_u(140, 420).min(glow.room());
            glow.hold(pause);
        } else if roll < 0.88 {
            let origin = glow.look.phase;
            let next = glow.face(1);
            let reach = rng.range(0.35, 0.72);
            let partial = origin + (next - origin) * reach;
            let sneak = rng.range_u(200, 480);
            glow.beam(&mut rng, partial, sneak, 1.36);
            glow.hold(rng.range_u(80, 280).min(glow.room()));
            let retreat = origin + (partial - origin) * rng.range(0.1, 0.35);
            let back = rng.range_u(140, 320);
            glow.beam(&mut rng, retreat, back, 1.3);
            glow.hold(rng.range_u(60, 200).min(glow.room()));
            let finish = rng.range_u(160, 420);
            glow.beam(&mut rng, next, finish, 1.22);
            glow.hold(rng.range_u(350, 1_400).min(glow.room()));
        } else {
            let back_to = glow.face(-1);
            let flick = rng.range_u(110, 240);
            glow.beam(&mut rng, back_to, flick, 1.22);
            glow.hold(rng.range_u(140, 520).min(glow.room()));
            let home = glow.face(1);
            let back = rng.range_u(140, 300);
            glow.beam(&mut rng, home, back, 1.26);
            let onward = glow.face(1);
            let ahead = rng.range_u(160, 380);
            glow.beam(&mut rng, onward, ahead, 1.26);
            glow.hold(rng.range_u(400, 1_400).min(glow.room()));
        }
        if glow.t == before {
            glow.hold(200.min(glow.room()));
        }
    }
    if glow.t < END_MS {
        glow.hold(END_MS - glow.t);
    }
    glow.keys
}

fn candle_show(seed: u64) -> Vec<GlowKey> {
    let mut rng = Rng(seed);
    let look = Look {
        phase: 0.0,
        width: 24.0,
        gain: 0.8,
        echo: 0.0,
    };
    let mut glow = Glow {
        keys: vec![GlowKey { t: 0, look }],
        t: 0,
        look,
        heading: 1.0,
        last_rate: 0.8,
        program: Program::Landscape,
    };
    let mut base = 0.78;
    while glow.room() > 40 {
        let roll = rng.f64();
        let (gain, dur) = if roll < 0.62 {
            base = (base + rng.range(-0.07, 0.07)).clamp(0.55, 0.92);
            let jitter = (base + rng.range(-0.2, 0.12)).clamp(0.26, 1.0);
            (jitter, rng.range_u(40, 120))
        } else if roll < 0.84 {
            let breath = (base + rng.range(-0.1, 0.08)).clamp(0.42, 0.96);
            (breath, rng.range_u(180, 480))
        } else if roll < 0.94 {
            (rng.range(0.92, 1.0), rng.range_u(40, 110))
        } else {
            (rng.range(0.1, 0.28), rng.range_u(70, 200))
        };
        glow.look = Look {
            phase: 0.0,
            width: 24.0,
            gain,
            echo: 0.0,
        };
        if glow.commit(dur) == 0 {
            break;
        }
    }
    if glow.t < END_MS {
        glow.hold(END_MS - glow.t);
    }
    glow.keys
}

fn sample_alt(keys: &[AltKey], t: u32) -> f64 {
    if t <= keys[0].t {
        return keys[0].pos;
    }
    let last = keys.last().unwrap();
    if t >= last.t {
        return last.pos;
    }
    let index = keys.partition_point(|key| key.t <= t) - 1;
    let start = &keys[index];
    let end = &keys[index + 1];
    if end.t <= start.t {
        return start.pos;
    }
    let u = f64::from(t - start.t) / f64::from(end.t - start.t);
    start.pos + (end.pos - start.pos) * u
}

fn sample_glow(keys: &[GlowKey], t: u32) -> Look {
    if t <= keys[0].t {
        return keys[0].look;
    }
    let last = keys.last().unwrap();
    if t >= last.t {
        return last.look;
    }
    let index = keys.partition_point(|key| key.t <= t) - 1;
    let start = &keys[index];
    let end = &keys[index + 1];
    if end.t <= start.t {
        return start.look;
    }
    let u = f64::from(t - start.t) / f64::from(end.t - start.t);
    Look {
        phase: start.look.phase + (end.look.phase - start.look.phase) * u,
        width: start.look.width + (end.look.width - start.look.width) * u,
        gain: start.look.gain + (end.look.gain - start.look.gain) * u,
        echo: start.look.echo + (end.look.echo - start.look.echo) * u,
    }
}

fn put16(slots: &mut [u8; 512], at: usize, value: u16) {
    slots[at] = (value >> 8) as u8;
    slots[at + 1] = (value & 0xff) as u8;
}

fn slots_at(altitude: &[AltKey], glow: &[GlowKey], t: u32) -> [u8; 512] {
    let mut slots = [0u8; 512];
    let pos = sample_alt(altitude, t).round().clamp(0.0, 65_535.0) as u16;
    put16(&mut slots, ALT, FULL - pos);
    put16(&mut slots, SPEED, FULL);
    put16(&mut slots, ACCEL, FULL);
    let levels = shade(sample_glow(glow, t));
    for (index, level) in levels.iter().enumerate() {
        put16(&mut slots, BULB + index * 2, *level);
    }
    slots
}

fn write_show(path: &PathBuf, altitude: &[AltKey], glow: &[GlowKey]) -> std::io::Result<()> {
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
        let slots = slots_at(altitude, glow, t);
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

fn confirm(path: &PathBuf, program: Program) {
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
        assert_eq!(&frame[406..410], &[0xff, 0xff, 0xff, 0xff], "speed at {at}");
        assert!(frame[402..406].iter().all(|byte| *byte == 0), "gap at {at}");
        if at == 0 || at == 300_000 {
            match program {
                Program::Discovery | Program::Mesocosme => {
                    if at == 0 {
                        let alt = u16::from_be_bytes([frame[400], frame[401]]);
                        assert_eq!(alt, FULL, "floor is not the bottom");
                    }
                    let mut levels = [0u16; 4];
                    for (index, level) in levels.iter_mut().enumerate() {
                        let slot = 300 + index * 2;
                        *level = u16::from_be_bytes([frame[slot], frame[slot + 1]]);
                    }
                    let max = levels.into_iter().max().unwrap();
                    assert!(max > 10_000, "{at} sphere is dark");
                }
                Program::Landscape => {
                    if at == 0 {
                        let alt = u16::from_be_bytes([frame[400], frame[401]]);
                        assert!(alt < 20_000, "landscape is not up, alt {alt}");
                    }
                    let mut levels = [0u16; 4];
                    for (index, level) in levels.iter_mut().enumerate() {
                        let slot = 300 + index * 2;
                        *level = u16::from_be_bytes([frame[slot], frame[slot + 1]]);
                    }
                    assert!(levels[0] > 1_000, "candle out at {at}");
                    assert!(
                        levels.iter().all(|level| *level == levels[0]),
                        "bulbs split at {at}"
                    );
                }
                Program::Phare => {
                    let alt = u16::from_be_bytes([frame[400], frame[401]]);
                    assert!(alt < 16_000, "phare is not up, alt {alt}");
                    if at == 0 {
                        let beam = u16::from_be_bytes([frame[300], frame[301]]);
                        assert!(beam > 50_000, "phare does not open on bulb 1");
                        assert!(frame[302..308].iter().all(|byte| *byte == 0));
                    }
                }
                Program::Steady => {
                    let alt = u16::from_be_bytes([frame[400], frame[401]]);
                    assert_eq!(alt, FULL - PERCH as u16);
                    let bulb = u16::from_be_bytes([frame[300], frame[301]]);
                    assert_eq!(bulb, FULL);
                    assert_eq!(
                        &frame[300..308],
                        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
                    );
                }
            }
        }
        frames += 1;
        pos += 19 + 512;
    }
    assert_eq!(frames, END_MS / PERIOD_MS + 1);
    assert_eq!(pos, data.len());
}
