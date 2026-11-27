pub fn format_ms(ms: u32) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes:02}:{seconds:02}.{millis:03}")
    }
}

pub fn parse_timecode(text: &str) -> Option<u32> {
    let text = text.trim();
    if let Ok(ms) = text.parse::<u32>() {
        return Some(ms);
    }
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        2 => {
            let minutes: u32 = parts[0].parse().ok()?;
            let sec_ms: Vec<&str> = parts[1].split('.').collect();
            let seconds: u32 = sec_ms.first()?.parse().ok()?;
            let millis: u32 = sec_ms.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            Some(minutes * 60_000 + seconds * 1_000 + millis)
        }
        3 => {
            let hours: u32 = parts[0].parse().ok()?;
            let minutes: u32 = parts[1].parse().ok()?;
            let sec_ms: Vec<&str> = parts[2].split('.').collect();
            let seconds: u32 = sec_ms.first()?.parse().ok()?;
            let millis: u32 = sec_ms.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            Some(hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + millis)
        }
        _ => None,
    }
}
