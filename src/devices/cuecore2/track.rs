use crate::show::{Frame, Show};

const SET_ONE: u8 = 0x10;
const SET_ONE_ZERO: u8 = 0x20;
const SET_RANGE: u8 = 0x30;
const SET_RANGE_ZERO: u8 = 0x40;
const ESCAPE: u8 = 0xc0;
const TICK: u8 = 0xd0;
const END: u8 = 0xe0;
const MAX_COUNT: usize = 255;

pub const DEFAULT_FPS: u32 = 40;

pub fn ms_per_frame(fps: u32) -> u32 {
    1_000 / fps.max(1)
}

pub fn decode(data: &[u8]) -> Result<Vec<[u8; 512]>, String> {
    let mut slots = [0u8; 512];
    let mut frames = Vec::new();
    let mut index = 0usize;
    let mut ended = false;
    while index < data.len() {
        let op = data[index];
        if op == END {
            if index + 1 != data.len() {
                return Err(format!(
                    "track continues after end marker at offset {index}"
                ));
            }
            ended = true;
            break;
        }
        if op == TICK {
            frames.push(slots);
            index += 1;
            continue;
        }
        let kind = op & 0xf0;
        let page = op & 0x0f;
        if page > 1 || !matches!(kind, SET_ONE | SET_ONE_ZERO | SET_RANGE | SET_RANGE_ZERO) {
            return Err(format!("unexpected byte 0x{op:02x} at offset {index}"));
        }
        index += 1;
        let address = read_logical(data, &mut index)?;
        let start = (page as usize) << 8 | address as usize;
        match kind {
            SET_ONE => {
                let value = read_logical(data, &mut index)?;
                slots[start] = value;
            }
            SET_ONE_ZERO => slots[start] = 0,
            SET_RANGE | SET_RANGE_ZERO => {
                let count = read_logical(data, &mut index)? as usize;
                if count == 0 || start + count > 512 {
                    return Err(format!(
                        "channel range {start}+{count} is outside 1..=512 at offset {index}"
                    ));
                }
                if kind == SET_RANGE_ZERO {
                    slots[start..start + count].fill(0);
                } else {
                    for slot in &mut slots[start..start + count] {
                        *slot = read_logical(data, &mut index)?;
                    }
                }
            }
            _ => return Err(format!("unexpected byte 0x{op:02x} at offset {index}")),
        }
    }
    if !ended {
        return Err("track has no end marker".into());
    }
    if frames.is_empty() {
        return Err("track has no frames".into());
    }
    Ok(frames)
}

fn read_logical(data: &[u8], index: &mut usize) -> Result<u8, String> {
    if *index >= data.len() {
        return Err("truncated track".into());
    }
    let byte = data[*index];
    *index += 1;
    if byte != ESCAPE {
        return Ok(byte);
    }
    if *index >= data.len() {
        return Err("truncated track".into());
    }
    let value = data[*index];
    *index += 1;
    Ok(value)
}

pub fn encode(frames: &[[u8; 512]]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut previous = [0u8; 512];
    for frame in frames {
        let mut channel = 0usize;
        while channel < 512 {
            if frame[channel] == previous[channel] {
                channel += 1;
                continue;
            }
            let start = channel;
            let page = start >> 8;
            channel += 1;
            while channel < 512
                && channel >> 8 == page
                && channel - start < MAX_COUNT
                && frame[channel] != previous[channel]
            {
                channel += 1;
            }
            write_range(&mut out, start, &frame[start..channel]);
        }
        previous = *frame;
        out.push(TICK);
    }
    out.push(END);
    out
}

fn write_range(out: &mut Vec<u8>, start: usize, values: &[u8]) {
    let page = (start >> 8) as u8;
    let address = (start & 0xff) as u8;
    if values.iter().all(|level| *level == 0) {
        if values.len() == 1 {
            out.push(SET_ONE_ZERO | page);
            write_logical(out, address);
        } else {
            out.push(SET_RANGE_ZERO | page);
            write_logical(out, address);
            write_logical(out, values.len() as u8);
        }
        return;
    }
    if values.len() == 1 {
        out.push(SET_ONE | page);
        write_logical(out, address);
        write_logical(out, values[0]);
        return;
    }
    out.push(SET_RANGE | page);
    write_logical(out, address);
    write_logical(out, values.len() as u8);
    for level in values {
        write_logical(out, *level);
    }
}

fn write_logical(out: &mut Vec<u8>, value: u8) {
    if matches!(value, ESCAPE | TICK | END) {
        out.push(ESCAPE);
    }
    out.push(value);
}

pub fn show_from_track(frames: &[[u8; 512]], fps: u32) -> Show {
    let step = ms_per_frame(fps);
    let mut sparse = Vec::new();
    let mut previous: Option<[u8; 512]> = None;
    for (index, slots) in frames.iter().enumerate() {
        if previous == Some(*slots) {
            continue;
        }
        if let Some(held) = previous {
            let hold_at = (index as u32).saturating_sub(1) * step;
            if sparse
                .last()
                .is_some_and(|frame: &Frame| frame.at_ms < hold_at)
            {
                sparse.push(Frame::full(hold_at, held));
            }
        }
        sparse.push(Frame::full(index as u32 * step, *slots));
        previous = Some(*slots);
    }
    let end_ms = frames.len().saturating_sub(1) as u32 * step;
    if sparse.last().is_some_and(|frame| frame.at_ms < end_ms) {
        let slots = sparse.last().map(|frame| frame.slots).unwrap_or([0; 512]);
        sparse.push(Frame::full(end_ms, slots));
    }
    Show::new(sparse)
}

pub fn track_from_show(show: &Show, fps: u32) -> Result<Vec<[u8; 512]>, String> {
    if show.frames.is_empty() {
        return Err("show has no frames".into());
    }
    if !show.timestamps_monotonic() {
        return Err("frame timestamps are not monotonic".into());
    }
    let step = ms_per_frame(fps);
    let end_ms = show.end_ms();
    let mut ticks = Vec::new();
    let mut show_index = 0usize;
    let mut current = show.frames[show_index].slots;
    let mut time = 0u32;
    while time <= end_ms {
        while show_index + 1 < show.frames.len() && show.frames[show_index + 1].at_ms <= time {
            show_index += 1;
            current = show.frames[show_index].slots;
        }
        ticks.push(current);
        time += step;
    }
    Ok(ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chezphilippe_track_has_expected_frame_count() {
        let blob = super::super::xml::track_payload_from_sample().unwrap();
        let frames = decode(&blob).unwrap();
        assert_eq!(frames.len(), 1_547);
        assert_eq!(frames[0][0], 175);
        assert!(frames[1_214].iter().all(|level| *level == 0));
        assert!(frames[1_318].iter().all(|level| *level == 0));
        assert_eq!(frames[1_319][0], 255);
        assert!(frames[1_414].iter().all(|level| *level == 0));
        assert!(frames[1_546].iter().all(|level| *level == 0));
    }

    #[test]
    fn round_trips_channel_values() {
        let blob = super::super::xml::track_payload_from_sample().unwrap();
        let frames = decode(&blob).unwrap();
        let encoded = encode(&frames);
        let again = decode(&encoded).unwrap();
        assert_eq!(frames, again);
    }

    #[test]
    fn ramp_first_second_changes_each_tick() {
        let blob = super::super::xml::track_payload_from_sample().unwrap();
        let frames = decode(&blob).unwrap();
        for index in 1..40 {
            assert_ne!(frames[index], frames[index - 1]);
        }
        assert_eq!(frames[0][0], 175);
        assert_eq!(frames[39][0], 170);
    }

    #[test]
    fn escapes_only_marker_bytes_inside_a_range() {
        let mut frame = [0u8; 512];
        frame[0] = 208;
        frame[1] = 255;
        let blob = encode(&[frame]);
        assert_eq!(blob, [0x30, 0x00, 0x02, 0xc0, 0xd0, 0xff, 0xd0, 0xe0]);
        assert_eq!(decode(&blob).unwrap(), vec![frame]);
    }

    #[test]
    fn writes_zero_with_the_short_opcodes() {
        let mut on = [0u8; 512];
        on[5] = 9;
        let one = encode(&[on, [0; 512]]);
        assert_eq!(one, [0x10, 0x05, 0x09, 0xd0, 0x20, 0x05, 0xd0, 0xe0]);
        on[6] = 9;
        let range = encode(&[on, [0; 512]]);
        assert_eq!(
            range,
            [0x30, 0x05, 0x02, 0x09, 0x09, 0xd0, 0x40, 0x05, 0x02, 0xd0, 0xe0]
        );
    }

    #[test]
    fn addresses_the_high_page_and_escapes_channel_192() {
        let mut frame = [0u8; 512];
        frame[192] = 3;
        frame[256] = 4;
        frame[511] = 5;
        let blob = encode(&[frame]);
        assert_eq!(
            blob,
            [0x10, 0xc0, 0xc0, 0x03, 0x11, 0x00, 0x04, 0x11, 0xff, 0x05, 0xd0, 0xe0]
        );
        assert_eq!(decode(&blob).unwrap(), vec![frame]);
    }
}
