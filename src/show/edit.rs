use super::{Frame, Show};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TrackPoint {
    pub at_ms: u32,
    pub level: u8,
}

#[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurveClear {
    Channel,
    Span,
    None,
}

pub fn validate_channel(channel: u16) -> Result<usize, String> {
    if !(1..=512).contains(&channel) {
        return Err(format!("channel {channel} is outside 1..512"));
    }
    Ok(usize::from(channel - 1))
}

pub fn track_keyframes(show: &Show, channel: u16) -> Result<Vec<TrackPoint>, String> {
    let slot = validate_channel(channel)?;
    let mut out = Vec::new();
    for frame in &show.frames {
        if frame.is_key(slot) {
            out.push(TrackPoint {
                at_ms: frame.at_ms,
                level: frame.slots[slot],
            });
        }
    }
    Ok(out)
}

pub fn sample_track(
    show: &Show,
    channel: u16,
    max_points: usize,
) -> Result<Vec<TrackPoint>, String> {
    let keyframes = track_keyframes(show, channel)?;
    let max_points = max_points.max(2).min(64);
    if keyframes.len() <= max_points {
        return Ok(keyframes);
    }
    let last = keyframes.len() - 1;
    let mut out = Vec::with_capacity(max_points);
    for index in 0..max_points {
        let source = index * last / (max_points - 1);
        out.push(keyframes[source].clone());
    }
    Ok(out)
}

pub fn write_channel_points(
    show: &mut Show,
    slot: usize,
    points: &[(u32, u8)],
    clear: CurveClear,
) -> usize {
    let points = dedup_points(points);
    let count = points.len();
    let clear_span = match clear {
        CurveClear::Channel => Some((0, u32::MAX)),
        CurveClear::Span => points
            .first()
            .map(|point| (point.0, points.last().unwrap().0)),
        CurveClear::None => None,
    };
    let clears = |at: u32| clear_span.is_some_and(|(start, end)| at >= start && at <= end);
    show.sort_by_time();
    let existing = std::mem::take(&mut show.frames);
    let mut frames = existing.into_iter().peekable();
    let mut incoming = points.into_iter().peekable();
    let mut out = Vec::new();
    loop {
        match (
            frames.peek().map(|frame| frame.at_ms),
            incoming.peek().copied(),
        ) {
            (None, None) => break,
            (Some(at), Some((time, _))) if at < time => {
                push_kept(&mut out, frames.next().unwrap(), slot, clears(at), None);
            }
            (Some(at), Some((time, level))) if at == time => {
                let _ = incoming.next();
                push_kept(
                    &mut out,
                    frames.next().unwrap(),
                    slot,
                    clears(at),
                    Some(level),
                );
            }
            (Some(_), Some((time, level))) => {
                let _ = incoming.next();
                let mut frame = Frame::blank(time);
                frame.set_key(slot, level);
                out.push(frame);
            }
            (Some(at), None) => {
                push_kept(&mut out, frames.next().unwrap(), slot, clears(at), None);
            }
            (None, Some((time, level))) => {
                let _ = incoming.next();
                let mut frame = Frame::blank(time);
                frame.set_key(slot, level);
                out.push(frame);
            }
        }
    }
    if out.is_empty() {
        out.push(Frame::blank(0));
    }
    show.frames = out;
    count
}

fn dedup_points(points: &[(u32, u8)]) -> Vec<(u32, u8)> {
    let mut ordered = points.to_vec();
    ordered.sort_by_key(|point| point.0);
    let mut out = Vec::with_capacity(ordered.len());
    for point in ordered {
        if out.last().is_some_and(|last: &(u32, u8)| last.0 == point.0) {
            out.pop();
        }
        out.push(point);
    }
    out
}

fn push_kept(out: &mut Vec<Frame>, mut frame: Frame, slot: usize, clear: bool, level: Option<u8>) {
    if clear {
        frame.clear_key(slot);
    }
    if let Some(level) = level {
        frame.set_key(slot, level);
    }
    if frame.any_key() {
        out.push(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_channel_points_merges_a_dense_curve() {
        let mut slots = [0u8; 512];
        slots[1] = 40;
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![
                Frame::full(0, slots),
                Frame::full(500, slots),
                Frame::full(1000, slots),
            ],
        };
        let count = write_channel_points(
            &mut show,
            0,
            &[(0, 0), (1000, 200), (500, 50)],
            CurveClear::Channel,
        );
        assert_eq!(count, 3);
        assert_eq!(
            track_keyframes(&show, 1).unwrap(),
            vec![
                TrackPoint { at_ms: 0, level: 0 },
                TrackPoint {
                    at_ms: 500,
                    level: 50
                },
                TrackPoint {
                    at_ms: 1000,
                    level: 200
                },
            ]
        );
        assert_eq!(show.value_at(1, 1000), 40);
        assert_eq!(show.value_at(0, 250), 25);
    }

    #[test]
    fn write_channel_points_span_keeps_keys_outside() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512])],
        };
        write_channel_points(
            &mut show,
            0,
            &[(0, 10), (500, 10), (1000, 10)],
            CurveClear::Channel,
        );
        write_channel_points(&mut show, 0, &[(200, 80), (400, 90)], CurveClear::Span);
        let keys = track_keyframes(&show, 1).unwrap();
        assert_eq!(
            keys.iter().map(|point| point.at_ms).collect::<Vec<_>>(),
            vec![0, 200, 400, 500, 1000]
        );
    }

    #[test]
    fn write_channel_points_none_overlays_and_last_duplicate_wins() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512])],
        };
        write_channel_points(&mut show, 0, &[(0, 10), (1000, 10)], CurveClear::Channel);
        write_channel_points(
            &mut show,
            0,
            &[(0, 1), (500, 200), (0, 9)],
            CurveClear::None,
        );
        assert_eq!(
            track_keyframes(&show, 1).unwrap(),
            vec![
                TrackPoint { at_ms: 0, level: 9 },
                TrackPoint {
                    at_ms: 500,
                    level: 200
                },
                TrackPoint {
                    at_ms: 1000,
                    level: 10
                },
            ]
        );
    }
}
