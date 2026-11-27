use crate::show::Show;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::path::Path;

use super::track;

const MARKER: &str = "device=\"CueCore2\"";

pub fn is_file(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.contains("<track ")
        && text.contains("sample_rate=\"")
        && (text.contains(MARKER) || text.contains("<tracks"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackChoice {
    pub index: u32,
    pub label: String,
    pub frames: u32,
    pub sample_rate: u32,
}

pub fn list_tracks(bytes: &[u8]) -> Result<Vec<TrackChoice>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "backup is not UTF-8".to_string())?;
    raw_tracks(text)?
        .into_iter()
        .map(|track| {
            Ok(TrackChoice {
                index: attribute(track.head, "index")?
                    .parse()
                    .map_err(|_| "track has an invalid index".to_string())?,
                label: optional_attribute(track.head, "label")
                    .unwrap_or("")
                    .to_string(),
                frames: attribute(track.head, "frames")?
                    .parse()
                    .map_err(|_| "track has an invalid frames".to_string())?,
                sample_rate: attribute(track.head, "sample_rate")?
                    .parse()
                    .map_err(|_| "track has an invalid sample_rate".to_string())?,
            })
        })
        .collect()
}

pub fn parse(bytes: &[u8]) -> Result<Show, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "backup is not UTF-8".to_string())?;
    let track = raw_tracks(text)?
        .into_iter()
        .next()
        .ok_or_else(|| "backup has no track".to_string())?;
    show_from_raw(&track)
}

pub fn parse_index(bytes: &[u8], index: u32) -> Result<Show, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "backup is not UTF-8".to_string())?;
    let track = raw_tracks(text)?.into_iter().find(|track| {
        attribute(track.head, "index")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            == Some(index)
    });
    let track = track.ok_or_else(|| format!("backup has no track {index}"))?;
    show_from_raw(&track)
}

pub fn to_bytes(show: &Show) -> Result<Vec<u8>, String> {
    let fps = track::DEFAULT_FPS;
    let channel_frames = track::track_from_show(show, fps)?;
    let payload = track::encode(&channel_frames);
    let encoded = STANDARD.encode(&payload);
    let xml = format!(
        "<?xml version=\"1.0\" ?>\n<tracks source=\"Art-Net\">\n    <track index=\"1\" version=\"1.46\" frames=\"{frames}\" filesize=\"{size}\" sample_rate=\"{fps}\">{encoded}</track>\n</tracks>\n",
        frames = channel_frames.len(),
        size = payload.len(),
    );
    Ok(xml.into_bytes())
}

pub fn track_payload_from_sample() -> Result<Vec<u8>, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/Cuecore2/chezphilippe.xml");
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("cannot read sample backup: {err}"))?;
    let track = raw_tracks(&text)?
        .into_iter()
        .next()
        .ok_or_else(|| "backup has no track".to_string())?;
    STANDARD
        .decode(track.body)
        .map_err(|err| format!("track is not valid base64: {err}"))
}

struct RawTrack<'a> {
    head: &'a str,
    body: &'a str,
}

fn raw_tracks(text: &str) -> Result<Vec<RawTrack<'_>>, String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find("<track ") {
        let open = cursor + rel;
        let head_end = text[open..]
            .find('>')
            .ok_or_else(|| "malformed track tag".to_string())?
            + open;
        let body_start = head_end + 1;
        let body_rel = text[body_start..]
            .find("</track>")
            .ok_or_else(|| "malformed track body".to_string())?;
        let body_end = body_start + body_rel;
        out.push(RawTrack {
            head: &text[open..head_end],
            body: text[body_start..body_end].trim(),
        });
        cursor = body_end + "</track>".len();
    }
    if out.is_empty() {
        return Err("backup has no track".into());
    }
    Ok(out)
}

fn show_from_raw(track: &RawTrack<'_>) -> Result<Show, String> {
    let fps = attribute(track.head, "sample_rate")?
        .parse()
        .map_err(|_| "track has an invalid sample_rate".to_string())?;
    let payload = STANDARD
        .decode(track.body)
        .map_err(|err| format!("track is not valid base64: {err}"))?;
    let frames = track::decode(&payload)?;
    Ok(track::show_from_track(&frames, fps))
}

fn attribute<'a>(head: &'a str, name: &str) -> Result<&'a str, String> {
    optional_attribute(head, name).ok_or_else(|| format!("track is missing {name}"))
}

fn optional_attribute<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = head.find(&needle)? + needle.len();
    let end = head[start..].find('"')? + start;
    Some(&head[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_chezphilippe_sparse_frames() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/Cuecore2/chezphilippe.xml");
        let show = parse(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(show.frames.len(), 1_220);
        assert_eq!(show.frames[0].at_ms, 0);
        assert_eq!(show.frames[0].slots[0], 175);
        assert_eq!(show.frames[0].slots[8], 0);
        assert_eq!(show.end_ms(), 38_650);
        let black = show
            .frames
            .iter()
            .find(|frame| frame.at_ms == 1_214 * 25)
            .unwrap();
        assert!(black.slots.iter().all(|level| *level == 0));
        let full = show
            .frames
            .iter()
            .find(|frame| frame.at_ms == 1_319 * 25)
            .unwrap();
        assert!(full.slots[..8].iter().all(|level| *level == 255));
    }

    #[test]
    fn decodes_every_track_in_the_second_backup() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/Cuecore2/chezphilippe2.xml");
        let text = std::fs::read_to_string(path).unwrap();
        let tracks = track_bodies(&text);
        assert_eq!(tracks.len(), 10);
        for (label, frames, blob) in &tracks {
            let decoded = track::decode(blob).unwrap_or_else(|err| panic!("{label}: {err}"));
            assert_eq!(decoded.len(), *frames, "{label}");
            assert_eq!(decoded, track::decode(&track::encode(&decoded)).unwrap());
        }
        let tracks: std::collections::HashMap<_, _> = tracks
            .into_iter()
            .map(|(label, _frames, blob)| (label, track::decode(&blob).unwrap()))
            .collect();
        assert!(tracks["Off"].iter().all(|frame| frame == &[0; 512]));
        assert!(tracks["Moving"]
            .iter()
            .all(|frame| frame[8..].iter().all(|level| *level == 0)));
        assert_eq!(
            tracks["Static"][0][..8],
            [137, 137, 137, 53, 137, 137, 137, 137]
        );
        assert!(tracks["Static"].windows(2).all(|pair| pair[0] == pair[1]));
        assert!(tracks["all512"][0].iter().all(|level| *level == 101));
        assert!(tracks["all512"][31].iter().all(|level| *level == 0));
        assert!(tracks["all512"][32].iter().all(|level| *level == 1));
        assert!(tracks["value208"]
            .iter()
            .all(|frame| frame.iter().all(|level| *level == 208)));
        assert!(tracks["nochangeandacha"][0].iter().all(|level| *level == 0));
        assert!(tracks["nochangeandacha"][208]
            .iter()
            .all(|level| *level == 255));
        assert!(tracks["nochangeandacha"][402]
            .iter()
            .all(|level| *level == 0));
        assert!(tracks["single channel"]
            .iter()
            .all(|frame| frame[1..].iter().all(|level| *level == 0)));
        assert_eq!(tracks["single channel"][0][0], 128);
        assert_eq!(tracks["single channel"][17][0], 192);
        assert!(tracks["single channel"].iter().any(|frame| frame[0] == 0));
        assert!(tracks["400to512"]
            .iter()
            .all(|frame| frame[..399].iter().all(|level| *level == 0)));
        assert!(tracks["400to512"][0][399..]
            .iter()
            .all(|level| *level == 238));
        assert!(tracks["fan512"]
            .iter()
            .all(|frame| frame[256..].iter().all(|level| *level == 0)));
        assert!(tracks["fan512quantized"].iter().all(|frame| {
            frame.iter().filter(|level| **level == 255).count() == 1
                && frame.iter().all(|level| *level == 0 || *level == 255)
        }));
        assert_eq!(
            tracks["fan512quantized"][0]
                .iter()
                .position(|level| *level == 255),
            Some(386)
        );
    }

    #[test]
    fn export_round_trips_through_track_payload() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/Cuecore2/chezphilippe.xml");
        let original = std::fs::read(&path).unwrap();
        let show = parse(&original).unwrap();
        let exported = to_bytes(&show).unwrap();
        let text = std::str::from_utf8(&exported).unwrap();
        assert!(is_file(&exported));
        assert!(text.starts_with("<?xml"));
        assert!(text.contains("<tracks"));
        assert!(!text.contains("device=\"CueCore2\""));
        assert!(!text.contains("<settings>"));
        assert_eq!(list_tracks(&exported).unwrap().len(), 1);
        let again = parse(&exported).unwrap();
        assert_eq!(show.frames.len(), again.frames.len());
        for (left, right) in show.frames.iter().zip(&again.frames) {
            assert_eq!(left.at_ms, right.at_ms);
            assert_eq!(left.slots, right.slots);
        }
    }

    #[test]
    fn lists_every_track_and_imports_the_chosen_one() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/Cuecore2/chezphilippe2.xml");
        let bytes = std::fs::read(path).unwrap();
        let tracks = list_tracks(&bytes).unwrap();
        assert_eq!(tracks.len(), 10);
        assert_eq!(tracks[0].index, 1);
        assert_eq!(tracks[0].label, "Off");
        assert_eq!(tracks[0].frames, 189);
        assert_eq!(tracks[0].sample_rate, 40);
        let all512 = tracks.iter().find(|track| track.label == "all512").unwrap();
        assert_eq!(all512.index, 17);
        let show = parse_index(&bytes, 3).unwrap();
        assert_eq!(show.frames[0].slots[0], 137);
        assert_eq!(show.frames[0].slots[3], 53);
        assert!(parse_index(&bytes, 99).is_err());
        let stepped = parse_index(&bytes, 19).unwrap();
        let step = 25;
        let keys: Vec<_> = stepped
            .frames
            .iter()
            .map(|frame| (frame.at_ms, frame.slots[0]))
            .collect();
        assert_eq!(
            keys,
            vec![
                (0, 0),
                (207 * step, 0),
                (208 * step, 255),
                (401 * step, 255),
                (402 * step, 0),
                (561 * step, 0),
                (562 * step, 255),
                (717 * step, 255),
                (718 * step, 0),
                (802 * step, 0),
            ]
        );
        assert_eq!(stepped.value_at(0, 100 * step), 0);
        assert_eq!(stepped.value_at(0, 300 * step), 255);
    }

    fn track_bodies(text: &str) -> Vec<(String, usize, Vec<u8>)> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find("<track ") {
            let head_end = rest[at..].find('>').unwrap() + at;
            let head = &rest[at..head_end];
            let body_start = head_end + 1;
            let body_end = rest[body_start..].find("</track>").unwrap() + body_start;
            let label = attribute(head, "label").unwrap_or("").to_string();
            let frames = attribute(head, "frames").unwrap().parse().unwrap();
            let data = STANDARD.decode(rest[body_start..body_end].trim()).unwrap();
            out.push((label, frames, data));
            rest = &rest[body_end + "</track>".len()..];
        }
        out
    }

    fn attribute<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        let needle = format!("{name}=\"");
        let start = head.find(&needle)? + needle.len();
        let end = head[start..].find('"')? + start;
        Some(&head[start..end])
    }
}
