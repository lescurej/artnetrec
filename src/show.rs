#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub at_ms: u32,
    pub slots: [u8; 512],
    pub keys: [u64; 8],
}

impl Frame {
    pub fn full(at_ms: u32, slots: [u8; 512]) -> Self {
        Self {
            at_ms,
            slots,
            keys: [u64::MAX; 8],
        }
    }

    pub fn blank(at_ms: u32) -> Self {
        Self {
            at_ms,
            slots: [0; 512],
            keys: [0; 8],
        }
    }

    pub fn is_key(&self, slot: usize) -> bool {
        slot < 512 && self.keys[slot / 64] & (1u64 << (slot % 64)) != 0
    }

    pub fn set_key(&mut self, slot: usize, level: u8) {
        if slot >= 512 {
            return;
        }
        self.slots[slot] = level;
        self.keys[slot / 64] |= 1u64 << (slot % 64);
    }

    pub fn clear_key(&mut self, slot: usize) {
        if slot >= 512 {
            return;
        }
        self.keys[slot / 64] &= !(1u64 << (slot % 64));
        self.slots[slot] = 0;
    }

    pub fn any_key(&self) -> bool {
        self.keys.iter().any(|word| *word != 0)
    }
}

#[derive(Debug, Clone)]
pub struct Show {
    pub frames: Vec<Frame>,
    pub output: crate::curve::ShowOutput,
}

impl Show {
    pub fn new(frames: Vec<Frame>) -> Self {
        Self {
            frames,
            output: crate::curve::ShowOutput::default(),
        }
    }

    pub fn end_ms(&self) -> u32 {
        self.frames.last().map(|frame| frame.at_ms).unwrap_or(0)
    }

    pub fn active_channels(&self) -> Vec<u16> {
        let mut mask = [false; 512];
        for frame in &self.frames {
            for (index, slot) in frame.slots.iter().enumerate() {
                if *slot != 0 {
                    mask[index] = true;
                }
            }
        }
        mask.iter()
            .enumerate()
            .filter_map(|(index, on)| on.then_some(index as u16 + 1))
            .collect()
    }

    pub fn timestamps_monotonic(&self) -> bool {
        self.frames
            .windows(2)
            .all(|pair| pair[0].at_ms <= pair[1].at_ms)
    }

    pub fn sort_by_time(&mut self) {
        self.frames.sort_by_key(|frame| frame.at_ms);
    }

    pub fn frame_index_at(&self, at_ms: u32) -> usize {
        index_at(&self.frames, at_ms)
    }

    pub fn value_at(&self, slot: usize, at_ms: u32) -> u8 {
        lerp_keys(&channel_keys(&self.frames, slot), at_ms)
    }

    pub fn levels_at(&self, at_ms: u32) -> [u8; 512] {
        levels_at(&self.frames, at_ms)
    }

    pub fn channel_keys(&self, slot: usize) -> Vec<(u32, u8)> {
        channel_keys(&self.frames, slot)
    }

    pub fn sparsify(&mut self) {
        let count = self.frames.len();
        if count == 0 {
            return;
        }
        let mut keep = vec![[0u64; 8]; count];
        for slot in 0..512 {
            let word = slot / 64;
            let bit = 1u64 << (slot % 64);
            keep[0][word] |= bit;
            if count > 1 {
                keep[count - 1][word] |= bit;
            }
            for index in 1..count.saturating_sub(1) {
                let level = self.frames[index].slots[slot];
                let previous = self.frames[index - 1].slots[slot];
                let next = self.frames[index + 1].slots[slot];
                if level != previous || level != next {
                    keep[index][word] |= bit;
                }
            }
        }
        for (frame, mask) in self.frames.iter_mut().zip(keep) {
            frame.keys = mask;
        }
    }

    pub fn frame_time_bounds(&self, index: usize) -> (u32, u32) {
        let min_ms = self
            .frames
            .get(index.wrapping_sub(1))
            .filter(|_| index > 0)
            .map(|frame| frame.at_ms.saturating_add(1))
            .unwrap_or(0);
        let max_ms = self
            .frames
            .get(index + 1)
            .map(|frame| frame.at_ms.saturating_sub(1))
            .unwrap_or(u32::MAX);
        (min_ms, max_ms.max(min_ms))
    }
}

pub mod edit;

pub fn index_at(frames: &[Frame], at_ms: u32) -> usize {
    frames
        .partition_point(|frame| frame.at_ms <= at_ms)
        .saturating_sub(1)
}

pub fn channel_keys(frames: &[Frame], slot: usize) -> Vec<(u32, u8)> {
    if slot >= 512 {
        return Vec::new();
    }
    frames
        .iter()
        .filter(|frame| frame.is_key(slot))
        .map(|frame| (frame.at_ms, frame.slots[slot]))
        .collect()
}

pub fn lerp_keys(keys: &[(u32, u8)], at_ms: u32) -> u8 {
    if keys.is_empty() {
        return 0;
    }
    if at_ms <= keys[0].0 {
        return keys[0].1;
    }
    if at_ms >= keys[keys.len() - 1].0 {
        return keys[keys.len() - 1].1;
    }
    let next = keys.partition_point(|key| key.0 < at_ms);
    let right = keys[next];
    let left = keys[next - 1];
    if right.0 == left.0 || at_ms == right.0 {
        return right.1;
    }
    let span = (right.0 - left.0) as f32;
    let t = (at_ms - left.0) as f32 / span;
    lerp_u8(left.1, right.1, t)
}

pub fn levels_at(frames: &[Frame], at_ms: u32) -> [u8; 512] {
    let mut levels = [0u8; 512];
    if frames.is_empty() {
        return levels;
    }
    let index = index_at(frames, at_ms);
    let left = &frames[index];
    if let Some(right) = frames.get(index + 1) {
        if left.keys == [u64::MAX; 8]
            && right.keys == [u64::MAX; 8]
            && right.at_ms > left.at_ms
            && at_ms >= left.at_ms
            && at_ms <= right.at_ms
        {
            let t = (at_ms - left.at_ms) as f32 / (right.at_ms - left.at_ms) as f32;
            for slot in 0..512 {
                levels[slot] = lerp_u8(left.slots[slot], right.slots[slot], t);
            }
            return levels;
        }
    } else if left.keys == [u64::MAX; 8] && at_ms >= left.at_ms {
        return left.slots;
    }
    for slot in 0..512 {
        levels[slot] = lerp_keys(&channel_keys(frames, slot), at_ms);
    }
    levels
}

pub fn index_tracks(frames: &[Frame]) -> Vec<Vec<(u32, u8)>> {
    let mut tracks = vec![Vec::new(); 512];
    for frame in frames {
        for word in 0..8 {
            let mut bits = frame.keys[word];
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let slot = word * 64 + bit;
                if slot < 512 {
                    tracks[slot].push((frame.at_ms, frame.slots[slot]));
                }
                bits &= bits - 1;
            }
        }
    }
    tracks
}

fn lerp_u8(from: u8, to: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (f32::from(from) + (f32::from(to) - f32::from(from)) * t).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_index_at_holds_previous_keyframe() {
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                Frame::full(0, [0; 512]),
                Frame::full(1000, [0; 512]),
                Frame::full(5000, [0; 512]),
            ],
        };
        assert_eq!(show.frame_index_at(0), 0);
        assert_eq!(show.frame_index_at(999), 0);
        assert_eq!(show.frame_index_at(1000), 1);
        assert_eq!(show.frame_index_at(4000), 1);
        assert_eq!(show.frame_index_at(5000), 2);
    }

    #[test]
    fn frame_time_bounds_stay_between_neighbors() {
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                Frame::full(0, [0; 512]),
                Frame::full(1000, [0; 512]),
                Frame::full(5000, [0; 512]),
            ],
        };
        assert_eq!(show.frame_time_bounds(0), (0, 999));
        assert_eq!(show.frame_time_bounds(1), (1, 4999));
        assert_eq!(show.frame_time_bounds(2), (1001, u32::MAX));
    }

    #[test]
    fn value_between_keys_is_linear() {
        let mut start = Frame::blank(0);
        start.set_key(0, 0);
        let mut end = Frame::blank(1000);
        end.set_key(0, 200);
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![start, end],
        };
        assert_eq!(show.value_at(0, 0), 0);
        assert_eq!(show.value_at(0, 500), 100);
        assert_eq!(show.value_at(0, 1000), 200);
        assert_eq!(show.value_at(0, 5000), 200);
    }

    #[test]
    fn sparsify_keeps_hold_endpoints() {
        let frames = (0..5)
            .map(|index| {
                let mut slots = [0u8; 512];
                slots[0] = if index < 4 { 0 } else { 255 };
                Frame::full(index * 20, slots)
            })
            .collect();
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames,
        };
        show.sparsify();
        let keys = show.channel_keys(0);
        assert_eq!(keys, vec![(0, 0), (60, 0), (80, 255)]);
        assert_eq!(show.value_at(0, 40), 0);
        assert_eq!(show.value_at(0, 70), 128);
    }
}
