use crate::devices;
use crate::show::{Frame, Show};
use std::fs;
use std::path::Path;

pub const EXTENSION: &str = "jsonl";

pub fn parse(bytes: &[u8]) -> Result<Show, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "show file is not UTF-8".to_string())?;
    parse_str(text.trim_start_matches('\u{feff}'))
}

pub fn parse_str(text: &str) -> Result<Show, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let Some((header_index, header)) = lines.next() else {
        return Err("show file is empty".into());
    };
    let header_line = header_index + 1;
    let header: serde_json::Value =
        serde_json::from_str(header).map_err(|err| format!("line {header_line}: {err}"))?;
    let header_obj = header
        .as_object()
        .ok_or_else(|| format!("line {header_line}: header must be an object"))?;
    let output = crate::curve::ShowOutput::from_header(header_obj)
        .map_err(|err| format!("line {header_line}: {err}"))?;
    let version = header
        .get("dmx-recorder")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("line {header_line}: missing \"dmx-recorder\" version"))?;
    if version != 1 && version != 2 {
        return Err(format!(
            "line {header_line}: unsupported dmx-recorder version {version}"
        ));
    }

    let mut frames = Vec::new();
    let mut previous_ms: Option<u32> = None;
    for (index, line) in lines {
        let line_no = index + 1;
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|err| format!("line {line_no}: {err}"))?;
        let at_ms = value
            .get("at_ms")
            .and_then(|item| item.as_u64())
            .ok_or_else(|| format!("line {line_no}: missing integer at_ms"))?;
        if at_ms > u64::from(u32::MAX) {
            return Err(format!("line {line_no}: at_ms is too large"));
        }
        let at_ms = at_ms as u32;
        if previous_ms.is_some_and(|previous| at_ms < previous) {
            return Err(format!("line {line_no}: at_ms goes backwards"));
        }
        previous_ms = Some(at_ms);

        let channels = value
            .get("channels")
            .and_then(|item| item.as_object())
            .ok_or_else(|| format!("line {line_no}: channels must be an object"))?;
        let mut frame = if version == 1 {
            Frame::full(at_ms, [0; 512])
        } else {
            Frame::blank(at_ms)
        };
        for (key, level) in channels {
            let channel: u16 = key
                .parse()
                .map_err(|_| format!("line {line_no}: bad channel {key}"))?;
            if !(1..=512).contains(&channel) {
                return Err(format!(
                    "line {line_no}: channel {channel} is outside 1..512"
                ));
            }
            let level = level.as_u64().ok_or_else(|| {
                format!("line {line_no}: channel {channel} level is not an integer")
            })?;
            if level > 255 {
                return Err(format!(
                    "line {line_no}: channel {channel} level {level} is above 255"
                ));
            }
            frame.set_key(usize::from(channel - 1), level as u8);
        }
        frames.push(frame);
    }

    if frames.is_empty() {
        return Err("show file has no frames".into());
    }
    let mut show = Show::new(frames);
    show.output = output;
    if version == 1 {
        show.sparsify();
    }
    Ok(show)
}

pub fn to_string(show: &Show) -> Result<String, String> {
    if show.frames.is_empty() {
        return Err("show has no frames".into());
    }
    if !show.timestamps_monotonic() {
        return Err("frame timestamps are not monotonic".into());
    }
    let mut out = show.output.header_line();
    out.push('\n');
    for frame in &show.frames {
        out.push_str("{\"at_ms\":");
        out.push_str(&frame.at_ms.to_string());
        out.push_str(",\"channels\":{");
        let mut first = true;
        for (index, level) in frame.slots.iter().enumerate() {
            if !frame.is_key(index) {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            out.push('"');
            out.push_str(&(index + 1).to_string());
            out.push_str("\":");
            out.push_str(&level.to_string());
        }
        out.push_str("}}\n");
    }
    Ok(out)
}

pub fn write(path: &Path, show: &Show) -> Result<(), String> {
    let text = to_string(show)?;
    fs::write(path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

pub fn parse_show(bytes: &[u8]) -> Result<Show, String> {
    for format in devices::show_formats() {
        if (format.detect)(bytes) {
            return (format.read)(bytes);
        }
    }
    parse(bytes)
}

pub fn write_show(path: &Path, show: &Show) -> Result<(), String> {
    if let Some(format) = format_for_path(path) {
        let bytes = (format.write)(show)?;
        return fs::write(path, bytes)
            .map_err(|err| format!("cannot write {}: {err}", path.display()));
    }
    write(path, show)
}

pub fn format_for_path(path: &Path) -> Option<&'static devices::ShowFormat> {
    let ext = path.extension()?.to_str()?;
    devices::show_formats().find(|format| {
        format
            .extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    })
}

pub fn with_extension(path: std::path::PathBuf) -> std::path::PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext)
            if ext.eq_ignore_ascii_case(EXTENSION)
                || devices::show_formats().any(|format| {
                    format
                        .extensions
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(ext))
                }) =>
        {
            path
        }
        _ => path.with_extension(EXTENSION),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_sparse_frames() {
        let mut first = [0u8; 512];
        first[1] = 255;
        let mut last = [0u8; 512];
        last[0] = 255;
        let show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, first), Frame::full(2000, last)],
        };
        let restored = parse_str(&to_string(&show).unwrap()).unwrap();
        assert_eq!(restored.frames.len(), 2);
        assert_eq!(restored.frames[0].slots[1], 255);
        assert_eq!(restored.frames[0].slots[0], 0);
        assert_eq!(restored.frames[1].slots[0], 255);
        assert_eq!(restored.frames[1].slots[1], 0);
        assert_eq!(restored.output.master, 255);
        assert!(!restored.output.explicit);
        assert!(to_string(&show)
            .unwrap()
            .starts_with("{\"dmx-recorder\":2}\n"));
    }

    #[test]
    fn header_keeps_master_and_transmission() {
        let mut show = Show::new(vec![Frame::full(0, [0; 512])]);
        show.output.master = 200;
        show.output.master_gamma = 2.0;
        show.output.channels = vec![(1, crate::curve::TransferLaw::Gamma { gamma: 2.0 }, 40)];
        show.output.explicit = true;
        let text = to_string(&show).unwrap();
        let header = text.lines().next().unwrap();
        assert!(header.contains("\"master\":200"), "{header}");
        assert!(header.contains("\"master_gamma\":2.0"), "{header}");
        assert!(header.contains("\"min_on\":40"), "{header}");
        let restored = parse_str(&text).unwrap();
        assert_eq!(restored.output.master, 200);
        assert_eq!(restored.output.master_gamma, 2.0);
        assert!(restored.output.explicit);
        assert_eq!(
            restored.output.channels,
            vec![(1, crate::curve::TransferLaw::Gamma { gamma: 2.0 }, 40)]
        );
    }

    #[test]
    fn example_matches_the_tr512_recording() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let native = parse(&fs::read(root.join("samples/example.jsonl")).unwrap()).unwrap();
        let device =
            crate::devices::tr512::ncod::load(&root.join("samples/Showtec TR-512/example.NCOD"))
                .unwrap();
        assert_eq!(native.frames.len(), device.frames.len());
        for (left, right) in native.frames.iter().zip(&device.frames) {
            assert_eq!(left.at_ms, right.at_ms);
            assert_eq!(left.slots, right.slots);
        }
    }

    #[test]
    fn rejects_a_backwards_time() {
        let text =
            "{\"dmx-recorder\":1}\n{\"at_ms\":10,\"channels\":{}}\n{\"at_ms\":9,\"channels\":{}}\n";
        assert!(parse_str(text).unwrap_err().contains("backwards"));
    }
}
