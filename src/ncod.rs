use std::fs;
use std::path::Path;

const MAGIC: &[u8] = b"framedmx";
pub const FILE_HEADER: usize = 512;
const PREFIX: usize = 19;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub at_ms: u32,
    pub slots: [u8; 512],
}

#[derive(Debug, Clone)]
pub struct Show {
    pub frames: Vec<Frame>,
}

impl Show {
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
}

pub fn compact_nslots(slots: &[u8; 512]) -> u16 {
    for index in (0..512).rev() {
        if slots[index] != 0 {
            return (index + 1) as u16;
        }
    }
    1
}

pub fn to_bytes(show: &Show) -> Result<Vec<u8>, String> {
    if show.frames.is_empty() {
        return Err("recording has no frames".into());
    }
    if !show.timestamps_monotonic() {
        return Err("frame timestamps are not monotonic".into());
    }

    let end_ms = show.end_ms();
    let mut data = vec![0u8; FILE_HEADER];
    data[4..8].copy_from_slice(&end_ms.to_be_bytes());
    data[8..12].copy_from_slice(&end_ms.to_be_bytes());
    data[12..14].copy_from_slice(&512u16.to_be_bytes());

    let mut prev_ms = 0u32;
    for frame in &show.frames {
        let gap = (frame.at_ms - prev_ms).saturating_mul(100);
        let nslots = compact_nslots(&frame.slots);
        data.extend_from_slice(MAGIC);
        data.push(0);
        data.extend_from_slice(&nslots.to_be_bytes());
        data.extend_from_slice(&gap.to_be_bytes());
        data.extend_from_slice(&frame.at_ms.to_be_bytes());
        data.extend_from_slice(&frame.slots[..usize::from(nslots)]);
        prev_ms = frame.at_ms;
    }
    Ok(data)
}

pub fn write(path: &Path, show: &Show) -> Result<(), String> {
    let data = to_bytes(show)?;
    fs::write(path, data).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

pub fn load(path: &Path) -> Result<Show, String> {
    let data = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    parse(&data)
}

pub fn parse(data: &[u8]) -> Result<Show, String> {
    if data.len() < FILE_HEADER + PREFIX || &data[FILE_HEADER..FILE_HEADER + MAGIC.len()] != MAGIC {
        return Err("not an NCOD recording".into());
    }

    let mut frames = Vec::new();
    let mut pos = FILE_HEADER;
    while pos < data.len() {
        if data.len() - pos < PREFIX || &data[pos..pos + MAGIC.len()] != MAGIC {
            return Err(format!("bad frame at offset {pos:#x}"));
        }
        let nslots = usize::from(u16_be(data, pos + 9));
        if !(1..=512).contains(&nslots) {
            return Err(format!("invalid slot count {nslots} at offset {pos:#x}"));
        }
        let at_ms = u32_be(data, pos + 15);
        let end = pos + PREFIX + nslots;
        if end > data.len() {
            return Err(format!("truncated frame at offset {pos:#x}"));
        }
        let mut slots = [0u8; 512];
        slots[..nslots].copy_from_slice(&data[pos + PREFIX..end]);
        frames.push(Frame { at_ms, slots });
        pos = end;
    }

    if frames.is_empty() {
        return Err("recording has no frames".into());
    }
    Ok(Show { frames })
}

fn u16_be(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn u32_be(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn append(buf: &mut Vec<u8>, nslots: u16, interval: u32, at_ms: u32, head: &[u8]) {
        buf.extend_from_slice(MAGIC);
        buf.push(0);
        buf.extend_from_slice(&nslots.to_be_bytes());
        buf.extend_from_slice(&interval.to_be_bytes());
        buf.extend_from_slice(&at_ms.to_be_bytes());
        buf.extend_from_slice(head);
        buf.extend(std::iter::repeat_n(0, usize::from(nslots) - head.len()));
    }

    #[test]
    fn parses_timestamps_and_slots() {
        let mut data = vec![0u8; FILE_HEADER];
        append(&mut data, 512, 2_200, 22, &[0x68, 0xad, 0xff, 0xa0]);
        append(&mut data, 511, 2_300, 45, &[0x68, 0xf6, 0xff, 0xa0]);
        let show = parse(&data).unwrap();
        assert_eq!(show.frames.len(), 2);
        assert_eq!(show.frames[0].at_ms, 22);
        assert_eq!(&show.frames[0].slots[..4], &[0x68, 0xad, 0xff, 0xa0]);
        assert_eq!(show.frames[1].at_ms, 45);
        assert_eq!(show.frames[1].slots[0], 0x68);
        assert_eq!(show.frames[1].slots[1], 0xf6);
        assert_eq!(show.frames[1].slots[511], 0);
        assert_eq!(show.end_ms(), 45);
    }

    #[test]
    fn rejects_a_shifted_frame() {
        let mut data = vec![0u8; FILE_HEADER];
        data.extend_from_slice(b"not-a-frame");
        assert!(parse(&data).is_err());
    }

    fn shows_equal(a: &Show, b: &Show) -> bool {
        if a.frames.len() != b.frames.len() {
            return false;
        }
        a.frames.iter().zip(&b.frames).all(|(left, right)| {
            left.at_ms == right.at_ms && left.slots == right.slots
        })
    }

    #[test]
    fn round_trips_sample_recording() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/SH-001.NCOD");
        let original = load(&path).unwrap();
        let blob = to_bytes(&original).unwrap();
        let restored = parse(&blob).unwrap();
        assert!(shows_equal(&original, &restored));
    }

    #[test]
    fn parses_the_sample_recording() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/SH-001.NCOD");
        let show = load(&path).unwrap();
        assert_eq!(show.frames.len(), 1207);
        assert_eq!(show.frames[0].at_ms, 22);
        assert_eq!(&show.frames[0].slots[..6], &[0x68, 0xad, 0xff, 0xa0, 0, 0]);
        assert_eq!(show.frames[1].at_ms, 45);
        assert_eq!(&show.frames[1].slots[..4], &[0x68, 0xf6, 0xff, 0xa0]);
        assert_eq!(show.frames[2].at_ms, 91);
        assert_eq!(&show.frames[2].slots[..2], &[0x69, 0x88]);
        let last = show.frames.last().unwrap();
        assert_eq!(last.at_ms, 43_739);
        assert_eq!(&last.slots[..2], &[0xac, 0xf1]);
        assert!(show.frames.iter().all(|frame| {
            frame.slots[2] == 0xff
                && frame.slots[3] == 0xa0
                && frame.slots[4] == 0
                && frame.slots[5] == 0
                && frame.slots[6..].iter().all(|b| *b == 0)
        }));
    }
}
