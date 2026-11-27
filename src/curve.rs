use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

pub const BULB_GAMMA: f32 = 0.30;
const VERSION: u32 = 1;
const GAMMA_MIN: f32 = 0.2;
const GAMMA_MAX: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CurvePoint {
    #[schemars(
        description = "Programmed level, 0..255. The first point is 0 and the last is 255."
    )]
    pub input: u8,
    #[schemars(description = "DMX byte sent for that programmed level.")]
    pub output: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "law", rename_all = "snake_case")]
#[schemars(
    description = "Maps a programmed level to the DMX byte sent to the fixture. linear sends the level unchanged. gamma is a power curve: 1 is linear, above 1 stays dark then rises, below 1 rises early. 0.3 matches a tungsten bulb whose light is not linear with voltage. s_curve is slow at both ends. points is piecewise linear and must start at input 0 and end at input 255."
)]
pub enum TransferLaw {
    Linear,
    Gamma {
        #[serde(serialize_with = "serialize_gamma")]
        gamma: f32,
    },
    SCurve,
    Points {
        points: Vec<CurvePoint>,
    },
}

fn serialize_gamma<S>(gamma: &f32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let hundredths = (*gamma * 100.0).round() as i64;
    serializer.serialize_f64(hundredths as f64 / 100.0)
}

impl Default for TransferLaw {
    fn default() -> Self {
        Self::Linear
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowOutput {
    pub master: u8,
    pub master_gamma: f32,
    pub channels: Vec<(u16, TransferLaw, u8)>,
    pub explicit: bool,
}

impl Default for ShowOutput {
    fn default() -> Self {
        Self {
            master: 255,
            master_gamma: 1.0,
            channels: Vec::new(),
            explicit: false,
        }
    }
}

impl ShowOutput {
    pub fn header_line(&self) -> String {
        let mut map = serde_json::Map::new();
        map.insert("dmx-recorder".into(), serde_json::json!(2));
        if self.master != 255 {
            map.insert("master".into(), serde_json::json!(self.master));
        }
        if self.master_gamma != 1.0 {
            let hundredths = (self.master_gamma * 100.0).round() as i64;
            map.insert(
                "master_gamma".into(),
                serde_json::json!(hundredths as f64 / 100.0),
            );
        }
        if !self.channels.is_empty() {
            let mut channels = serde_json::Map::new();
            for (channel, law, min_on) in &self.channels {
                if let Ok(value) = stored_value(law, *min_on) {
                    channels.insert(channel.to_string(), value);
                }
            }
            if !channels.is_empty() {
                map.insert("transmission".into(), serde_json::Value::Object(channels));
            }
        }
        serde_json::Value::Object(map).to_string()
    }

    pub fn from_header(
        header: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, String> {
        let explicit = header.contains_key("master")
            || header.contains_key("master_gamma")
            || header.contains_key("transmission");
        let master = match header.get("master") {
            None => 255,
            Some(value) => {
                let level = value
                    .as_u64()
                    .ok_or_else(|| "master must be an integer".to_string())?;
                if level > 255 {
                    return Err("master is above 255".into());
                }
                level as u8
            }
        };
        let master_gamma = match header.get("master_gamma") {
            None => 1.0,
            Some(value) => {
                let gamma = value
                    .as_f64()
                    .ok_or_else(|| "master gamma must be a number".to_string())?
                    as f32;
                let _ = normalize(TransferLaw::Gamma { gamma })?;
                (gamma * 100.0).round() / 100.0
            }
        };
        let mut channels = Vec::new();
        if let Some(stored) = header.get("transmission") {
            let map = stored
                .as_object()
                .ok_or_else(|| "transmission must be an object".to_string())?;
            for (key, value) in map {
                let channel: u16 = key
                    .parse()
                    .map_err(|_| format!("bad transmission channel {key}"))?;
                if !(1..=512).contains(&channel) {
                    return Err(format!("channel {channel} is outside 1..512"));
                }
                let min_on = value
                    .get("min_on")
                    .and_then(|item| item.as_u64())
                    .unwrap_or(0);
                if min_on > 255 {
                    return Err(format!("channel {channel}: min_on is above 255"));
                }
                let law: TransferLaw = serde_json::from_value(value.clone())
                    .map_err(|err| format!("channel {channel}: {err}"))?;
                channels.push((channel, normalize(law)?, min_on as u8));
            }
            channels.sort_by_key(|(channel, _, _)| *channel);
        }
        Ok(Self {
            master,
            master_gamma,
            channels,
            explicit,
        })
    }
}

impl TransferLaw {
    pub fn is_linear(&self) -> bool {
        matches!(self, Self::Linear)
    }

    pub fn gamma_value(&self) -> Option<f32> {
        match self {
            Self::Linear => Some(1.0),
            Self::Gamma { gamma } => Some(*gamma),
            _ => None,
        }
    }

    pub fn lut(&self) -> [u8; 256] {
        match self {
            Self::Linear => identity_lut(),
            Self::Gamma { gamma } => lut_gamma(*gamma),
            Self::SCurve => lut_smoothstep(),
            Self::Points { points } => lut_points(points),
        }
    }
}

pub fn bake_min_on(lut: &[u8; 256], min_on: u8) -> [u8; 256] {
    if min_on == 0 {
        return *lut;
    }
    let mut out = [0u8; 256];
    let span = u16::from(255 - min_on);
    for input in 1..256 {
        out[input] = (u16::from(min_on) + (u16::from(lut[input]) * span + 127) / 255) as u8;
    }
    out
}

pub fn apply_min_on(input: u8, shaped: u8, min_on: u8) -> u8 {
    if input == 0 || min_on == 0 {
        if min_on > 0 {
            0
        } else {
            shaped
        }
    } else {
        let span = u16::from(255 - min_on);
        (u16::from(min_on) + (u16::from(shaped) * span + 127) / 255) as u8
    }
}

pub fn unbake_min_on(sent: u8, min_on: u8) -> u8 {
    if min_on == 0 {
        return sent;
    }
    if sent <= min_on {
        return 0;
    }
    let span = u16::from(255 - min_on);
    let above = u16::from(sent - min_on);
    ((above * 255 + span / 2) / span).min(255) as u8
}

fn identity_lut() -> [u8; 256] {
    let mut lut = [0u8; 256];
    for input in 0..256 {
        lut[input] = input as u8;
    }
    lut
}

fn lut_gamma(gamma: f32) -> [u8; 256] {
    let gamma = f64::from(gamma);
    let mut lut = [0u8; 256];
    for input in 0..256 {
        let t = input as f64 / 255.0;
        lut[input] = (t.powf(gamma) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

fn lut_smoothstep() -> [u8; 256] {
    let mut lut = [0u8; 256];
    for input in 0..256 {
        let t = input as f64 / 255.0;
        let shaped = t * t * (3.0 - 2.0 * t);
        lut[input] = (shaped * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

fn lut_points(points: &[CurvePoint]) -> [u8; 256] {
    if points.len() < 2 {
        return identity_lut();
    }
    let mut lut = [0u8; 256];
    for window in points.windows(2) {
        let left = window[0];
        let right = window[1];
        let span = i32::from(right.input) - i32::from(left.input);
        if span <= 0 {
            continue;
        }
        for input in left.input..=right.input {
            let t = i32::from(input) - i32::from(left.input);
            let y = i32::from(left.output) * (span - t) + i32::from(right.output) * t;
            lut[usize::from(input)] = ((y + span / 2) / span) as u8;
        }
    }
    lut
}

pub fn normalize(law: TransferLaw) -> Result<TransferLaw, String> {
    match law {
        TransferLaw::Linear => Ok(TransferLaw::Linear),
        TransferLaw::SCurve => Ok(TransferLaw::SCurve),
        TransferLaw::Gamma { gamma } => {
            if !gamma.is_finite() {
                return Err("gamma must be a finite number".into());
            }
            let gamma = (gamma * 100.0).round() / 100.0;
            if !(GAMMA_MIN..=GAMMA_MAX).contains(&gamma) {
                return Err(format!("gamma must be between {GAMMA_MIN} and {GAMMA_MAX}"));
            }
            if (gamma - 1.0).abs() < 0.005 {
                Ok(TransferLaw::Linear)
            } else {
                Ok(TransferLaw::Gamma { gamma })
            }
        }
        TransferLaw::Points { points } => {
            if !(2..=12).contains(&points.len()) {
                return Err("a curve needs 2 to 12 points".into());
            }
            if points[0].input != 0 || points.last().map(|point| point.input) != Some(255) {
                return Err("curve points must start at input 0 and end at input 255".into());
            }
            for pair in points.windows(2) {
                if pair[1].input <= pair[0].input {
                    return Err("curve inputs must increase".into());
                }
            }
            Ok(TransferLaw::Points { points })
        }
    }
}

pub fn seed_points(law: &TransferLaw) -> Vec<CurvePoint> {
    let lut = law.lut();
    [0u8, 64, 128, 192, 255]
        .into_iter()
        .map(|input| CurvePoint {
            input,
            output: lut[usize::from(input)],
        })
        .collect()
}

pub fn move_point(points: &mut [CurvePoint], index: usize, x: u8, y: u8) {
    let last = points.len().saturating_sub(1);
    if index >= points.len() {
        return;
    }
    if index == 0 || index == last {
        points[index].output = y;
        return;
    }
    let min_x = points[index - 1].input.saturating_add(1);
    let max_x = points[index + 1].input.saturating_sub(1);
    if min_x <= max_x {
        points[index].input = x.clamp(min_x, max_x);
    }
    points[index].output = y;
}

pub fn insert_point(points: &mut Vec<CurvePoint>, x: u8, y: u8) -> usize {
    if let Some(index) = points.iter().position(|point| point.input == x) {
        points[index].output = y;
        return index;
    }
    if points.len() >= 12 {
        let index = nearest_input(points, x);
        move_point(points, index, x, y);
        return index;
    }
    let index = points
        .iter()
        .position(|point| point.input > x)
        .unwrap_or(points.len());
    points.insert(
        index,
        CurvePoint {
            input: x,
            output: y,
        },
    );
    index
}

pub fn remove_point(points: &mut Vec<CurvePoint>, index: usize) -> bool {
    if index == 0 || index + 1 >= points.len() || points.len() <= 2 {
        return false;
    }
    points.remove(index);
    true
}

fn nearest_input(points: &[CurvePoint], x: u8) -> usize {
    points
        .iter()
        .enumerate()
        .min_by_key(|(_, point)| point.input.abs_diff(x))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

struct Inner {
    laws: Vec<TransferLaw>,
    min_on: Vec<u8>,
    luts: Vec<[u8; 256]>,
    master_gamma: f32,
    master_lut: [u8; 256],
}

pub struct TrackCurves {
    inner: RwLock<Inner>,
    generation: AtomicU64,
}

impl TrackCurves {
    pub fn new() -> Self {
        let laws = vec![TransferLaw::Linear; 512];
        let min_on = vec![0u8; 512];
        let luts = laws.iter().map(TransferLaw::lut).collect();
        Self {
            inner: RwLock::new(Inner {
                laws,
                min_on,
                luts,
                master_gamma: 1.0,
                master_lut: TransferLaw::Linear.lut(),
            }),
            generation: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn map_master(&self, level: u8) -> u8 {
        let inner = self.read();
        if inner.master_gamma == 1.0 {
            level
        } else {
            inner.master_lut[usize::from(level)]
        }
    }

    pub fn master_gamma(&self) -> f32 {
        self.read().master_gamma
    }

    pub fn set_master_gamma(&self, gamma: f32) -> Result<(), String> {
        let gamma = match normalize(TransferLaw::Gamma { gamma })? {
            TransferLaw::Linear => 1.0,
            TransferLaw::Gamma { gamma } => gamma,
            _ => return Err("master gamma is not a power".into()),
        };
        let lut = if gamma == 1.0 {
            TransferLaw::Linear.lut()
        } else {
            TransferLaw::Gamma { gamma }.lut()
        };
        {
            let mut inner = self.write();
            inner.master_gamma = gamma;
            inner.master_lut = lut;
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn law(&self, channel: u16) -> TransferLaw {
        let Ok(slot) = slot_index(channel) else {
            return TransferLaw::Linear;
        };
        self.read().laws[slot].clone()
    }

    pub fn min_on(&self, channel: u16) -> u8 {
        let Ok(slot) = slot_index(channel) else {
            return 0;
        };
        self.read().min_on[slot]
    }

    pub fn is_shaped(&self, channel: u16) -> bool {
        let Ok(slot) = slot_index(channel) else {
            return false;
        };
        let inner = self.read();
        !inner.laws[slot].is_linear() || inner.min_on[slot] > 0
    }

    pub fn lookup(&self, channel: u16, level: u8) -> u8 {
        let Ok(slot) = slot_index(channel) else {
            return level;
        };
        let inner = self.read();
        if inner.laws[slot].is_linear() && inner.min_on[slot] == 0 {
            level
        } else {
            inner.luts[slot][usize::from(level)]
        }
    }

    pub fn set(&self, channel: u16, law: TransferLaw) -> Result<(), String> {
        let slot = slot_index(channel)?;
        let law = normalize(law)?;
        {
            let mut inner = self.write();
            let min_on = inner.min_on[slot];
            inner.luts[slot] = bake_min_on(&law.lut(), min_on);
            inner.laws[slot] = law;
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn set_min_on(&self, channel: u16, min_on: u8) -> Result<(), String> {
        let slot = slot_index(channel)?;
        {
            let mut inner = self.write();
            let lut = inner.laws[slot].lut();
            inner.luts[slot] = bake_min_on(&lut, min_on);
            inner.min_on[slot] = min_on;
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn apply(&self, slots: &mut [u8; 512]) {
        let inner = self.read();
        for (index, slot) in slots.iter_mut().enumerate() {
            if !inner.laws[index].is_linear() || inner.min_on[index] > 0 {
                *slot = inner.luts[index][usize::from(*slot)];
            }
        }
    }

    pub fn export_json(&self) -> serde_json::Value {
        let mut channels = serde_json::Map::new();
        for (channel, law, min_on) in self.shaped() {
            if let Ok(value) = stored_value(&law, min_on) {
                channels.insert(channel.to_string(), value);
            }
        }
        serde_json::Value::Object(channels)
    }

    pub fn channel_json(&self, channel: u16) -> serde_json::Value {
        stored_value(&self.law(channel), self.min_on(channel)).unwrap_or(serde_json::Value::Null)
    }

    pub fn snapshot(&self) -> ShowOutput {
        ShowOutput {
            master: 255,
            master_gamma: self.master_gamma(),
            channels: self.shaped(),
            explicit: true,
        }
    }

    pub fn restore(&self, output: &ShowOutput) -> Result<(), String> {
        let channels = output
            .channels
            .iter()
            .cloned()
            .map(|(channel, law, min_on)| (channel, (law, min_on)))
            .collect();
        self.replace(channels)?;
        self.set_master_gamma(output.master_gamma)
    }

    pub fn is_plain(&self) -> bool {
        self.master_gamma() == 1.0 && self.shaped().is_empty()
    }

    fn shaped(&self) -> Vec<(u16, TransferLaw, u8)> {
        let inner = self.read();
        inner
            .laws
            .iter()
            .enumerate()
            .filter_map(|(index, law)| {
                let min_on = inner.min_on[index];
                if law.is_linear() && min_on == 0 {
                    None
                } else {
                    Some(((index as u16) + 1, law.clone(), min_on))
                }
            })
            .collect()
    }

    pub fn replace(&self, channels: BTreeMap<u16, (TransferLaw, u8)>) -> Result<(), String> {
        let mut laws = vec![TransferLaw::Linear; 512];
        let mut min_on = vec![0u8; 512];
        for (channel, (law, strike)) in channels {
            let slot = slot_index(channel)?;
            laws[slot] = normalize(law)?;
            min_on[slot] = strike;
        }
        let kept = {
            let inner = self.read();
            (inner.master_gamma, inner.master_lut)
        };
        let luts = laws
            .iter()
            .zip(&min_on)
            .map(|(law, strike)| bake_min_on(&law.lut(), *strike))
            .collect();
        {
            let mut inner = self.write();
            inner.laws = laws;
            inner.min_on = min_on;
            inner.luts = luts;
            inner.master_gamma = kept.0;
            inner.master_lut = kept.1;
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, String> {
        let mut channels = serde_json::Map::new();
        for (channel, law, min_on) in self.shaped() {
            channels.insert(channel.to_string(), stored_value(&law, min_on)?);
        }
        let mut root = serde_json::json!({
            "version": VERSION,
            "channels": channels,
        });
        let gamma = self.master_gamma();
        if gamma != 1.0 {
            let hundredths = (gamma * 100.0).round() as i64;
            root["master_gamma"] = serde_json::json!(hundredths as f64 / 100.0);
        }
        serde_json::to_string_pretty(&root).map_err(|err| format!("serialize curves: {err}"))
    }

    pub fn load_json(&self, text: &str) -> Result<(), String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|err| format!("curves: {err}"))?;
        let version = value
            .get("version")
            .and_then(|item| item.as_u64())
            .ok_or_else(|| "curves file is missing a version".to_string())?;
        if version != u64::from(VERSION) {
            return Err(format!("unsupported curves version {version}"));
        }
        let mut channels = BTreeMap::new();
        if let Some(map) = value.get("channels").and_then(|item| item.as_object()) {
            for (key, stored) in map {
                let channel: u16 = key
                    .parse()
                    .map_err(|_| format!("bad curve channel {key}"))?;
                let min_on = stored
                    .get("min_on")
                    .and_then(|item| item.as_u64())
                    .unwrap_or(0);
                if min_on > 255 {
                    return Err(format!("channel {channel}: min_on is above 255"));
                }
                let law = serde_json::from_value(stored.clone())
                    .map_err(|err| format!("channel {channel}: {err}"))?;
                channels.insert(channel, (law, min_on as u8));
            }
        }
        self.replace(channels)?;
        let gamma = value
            .get("master_gamma")
            .and_then(|item| item.as_f64())
            .unwrap_or(1.0);
        self.set_master_gamma(gamma as f32)
    }

    pub fn load_sidecar(&self, show_path: &Path) -> Result<(), String> {
        let path = sidecar_path(show_path);
        if !path.exists() {
            return self.replace(BTreeMap::new());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        self.load_json(&text)
            .map_err(|err| format!("{}: {err}", path.display()))
    }

    pub fn save_sidecar(&self, show_path: &Path) -> Result<(), String> {
        let path = sidecar_path(show_path);
        if self.shaped().is_empty() && self.master_gamma() == 1.0 {
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|err| format!("cannot remove {}: {err}", path.display()))?;
            }
            return Ok(());
        }
        let text = self.to_json()?;
        std::fs::write(&path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|err| err.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|err| err.into_inner())
    }
}

impl Default for TrackCurves {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sidecar_path(show_path: &Path) -> PathBuf {
    let mut path = show_path.with_extension("");
    path.set_extension("curves.json");
    path
}

fn stored_value(law: &TransferLaw, min_on: u8) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(law).map_err(|err| format!("serialize curve: {err}"))?;
    if min_on > 0 {
        value
            .as_object_mut()
            .ok_or_else(|| "curve is not an object".to_string())?
            .insert("min_on".into(), serde_json::json!(min_on));
    }
    Ok(value)
}

fn slot_index(channel: u16) -> Result<usize, String> {
    if (1..=512).contains(&channel) {
        Ok(usize::from(channel - 1))
    } else {
        Err(format!("channel {channel} is outside 1..512"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_stays_low_and_inverse_rises_early() {
        let square = TransferLaw::Gamma { gamma: 2.0 }.lut();
        assert_eq!(square[0], 0);
        assert_eq!(square[255], 255);
        assert_eq!(square[128], 64);
        assert!(square[64] < 20);

        let inverse = TransferLaw::Gamma { gamma: 0.5 }.lut();
        assert_eq!(inverse[64], 128);
        assert!(inverse[128] > 180);

        let bulb = TransferLaw::Gamma { gamma: BULB_GAMMA }.lut();
        assert_eq!(bulb[0], 0);
        assert_eq!(bulb[255], 255);
        assert!(bulb[128] > 190);
        assert!(bulb[64] > 64);
    }

    #[test]
    fn preheat_holds_at_zero_and_midpoint_is_linear() {
        let law = TransferLaw::Points {
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
        };
        let lut = law.lut();
        assert_eq!(lut[0], 40);
        assert_eq!(lut[255], 255);
        let linear = TransferLaw::Points {
            points: vec![
                CurvePoint {
                    input: 0,
                    output: 0,
                },
                CurvePoint {
                    input: 255,
                    output: 255,
                },
            ],
        };
        assert_eq!(linear.lut()[128], 128);
    }

    #[test]
    fn points_cannot_cross_or_leave_the_ends() {
        let mut points = seed_points(&TransferLaw::Linear);
        move_point(&mut points, 2, 0, 10);
        assert!(points[2].input > points[1].input);
        assert!(points[2].input < points[3].input);
        move_point(&mut points, 0, 80, 25);
        assert_eq!(points[0].input, 0);
        assert_eq!(points[0].output, 25);
        assert!(!remove_point(&mut points, 0));
        assert!(remove_point(&mut points, 2));
    }

    #[test]
    fn json_round_trip_drops_linear_channels() {
        let curves = TrackCurves::new();
        curves
            .set(2, TransferLaw::Gamma { gamma: BULB_GAMMA })
            .unwrap();
        curves
            .set(
                4,
                TransferLaw::Points {
                    points: vec![
                        CurvePoint {
                            input: 0,
                            output: 10,
                        },
                        CurvePoint {
                            input: 255,
                            output: 255,
                        },
                    ],
                },
            )
            .unwrap();
        let text = curves.to_json().unwrap();
        assert!(text.contains("\"2\""));
        assert!(!text.contains("\"1\""));
        let restored = TrackCurves::new();
        restored.set(1, TransferLaw::Gamma { gamma: 2.0 }).unwrap();
        restored.load_json(&text).unwrap();
        assert!(restored.law(1).is_linear());
        assert!(restored.lookup(2, 128) > 190);
        assert_eq!(restored.lookup(4, 0), 10);
        assert_eq!(restored.lookup(3, 80), 80);
    }

    #[test]
    fn gamma_one_is_linear_and_out_of_range_is_refused() {
        let curves = TrackCurves::new();
        curves.set(1, TransferLaw::Gamma { gamma: 1.0 }).unwrap();
        assert!(curves.law(1).is_linear());
        assert!(curves.set(1, TransferLaw::Gamma { gamma: 0.0 }).is_err());
        assert!(curves
            .set(1, TransferLaw::Gamma { gamma: f32::NAN })
            .is_err());
        assert!(curves.set(0, TransferLaw::Linear).is_err());
        assert!(curves.law(1).is_linear());
    }

    #[test]
    fn apply_changes_only_shaped_channels() {
        let curves = TrackCurves::new();
        curves.set(1, TransferLaw::Gamma { gamma: 2.0 }).unwrap();
        let mut slots = [0u8; 512];
        slots[0] = 128;
        slots[1] = 128;
        curves.apply(&mut slots);
        assert_eq!(slots[0], 64);
        assert_eq!(slots[1], 128);
    }

    #[test]
    fn sidecar_uses_the_show_stem() {
        assert_eq!(
            sidecar_path(Path::new("/tmp/show.jsonl")),
            PathBuf::from("/tmp/show.curves.json")
        );
    }

    #[test]
    fn minimum_on_strikes_above_zero_and_full_stays_full() {
        let curves = TrackCurves::new();
        curves.set_min_on(1, 40).unwrap();
        assert_eq!(curves.lookup(1, 0), 0);
        assert_eq!(curves.lookup(1, 1), 41);
        assert_eq!(curves.lookup(1, 255), 255);
        assert!(curves.is_shaped(1));
        curves.set(1, TransferLaw::Gamma { gamma: 2.0 }).unwrap();
        assert_eq!(curves.min_on(1), 40);
        assert_eq!(curves.lookup(1, 0), 0);
        assert_eq!(curves.lookup(1, 1), 40);
        assert_eq!(curves.lookup(1, 255), 255);
        let text = curves.to_json().unwrap();
        assert!(text.contains("\"min_on\": 40"));
        let restored = TrackCurves::new();
        restored.load_json(&text).unwrap();
        assert_eq!(restored.min_on(1), 40);
        assert_eq!(restored.lookup(1, 0), 0);
        assert_eq!(restored.lookup(1, 255), 255);
    }

    #[test]
    fn master_gamma_shapes_the_fader_and_keeps_the_ends() {
        let curves = TrackCurves::new();
        assert_eq!(curves.map_master(128), 128);
        curves.set_master_gamma(2.0).unwrap();
        assert_eq!(curves.map_master(0), 0);
        assert_eq!(curves.map_master(128), 64);
        assert_eq!(curves.map_master(255), 255);
        curves.set_master_gamma(1.0).unwrap();
        assert_eq!(curves.master_gamma(), 1.0);
        assert_eq!(curves.map_master(128), 128);
        curves.set_master_gamma(BULB_GAMMA).unwrap();
        let text = curves.to_json().unwrap();
        assert!(text.contains("master_gamma"));
        let restored = TrackCurves::new();
        restored.load_json(&text).unwrap();
        assert!((restored.master_gamma() - BULB_GAMMA).abs() < 0.011);
        assert!(restored.map_master(128) > 190);
        restored
            .load_json(r#"{"version":1,"channels":{}}"#)
            .unwrap();
        assert_eq!(restored.master_gamma(), 1.0);
    }
}
