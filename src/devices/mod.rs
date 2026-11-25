use crate::show::Show;

pub mod tr512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Supported,
}

#[derive(Clone, Copy)]
pub struct ShowFormat {
    pub label: &'static str,
    pub extension: &'static str,
    pub extensions: &'static [&'static str],
    pub detect: fn(&[u8]) -> bool,
    pub read: fn(&[u8]) -> Result<Show, String>,
    pub write: fn(&Show) -> Result<Vec<u8>, String>,
}

#[derive(Clone, Copy)]
pub struct Device {
    pub name: &'static str,
    pub code: Option<&'static str>,
    pub status: Status,
    pub detail: &'static str,
    pub show: Option<ShowFormat>,
    pub config_extensions: &'static [&'static str],
}

pub const DEVICES: &[Device] = &[tr512::DEVICE];

pub fn show_formats() -> impl Iterator<Item = &'static ShowFormat> {
    DEVICES.iter().filter_map(|device| device.show.as_ref())
}

pub fn open_filters() -> Vec<(&'static str, Vec<&'static str>)> {
    DEVICES
        .iter()
        .filter_map(|device| {
            let mut extensions = Vec::new();
            if let Some(show) = &device.show {
                extensions.extend_from_slice(show.extensions);
            }
            extensions.extend_from_slice(device.config_extensions);
            if extensions.is_empty() {
                None
            } else {
                Some((device.name, extensions))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr512_detects_its_show_and_ignores_jsonl() {
        let format = tr512::DEVICE.show.unwrap();
        let ncod = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("samples/Showtec TR-512/example.NCOD"),
        )
        .unwrap();
        let jsonl = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/example.jsonl"),
        )
        .unwrap();
        assert!((format.detect)(&ncod));
        assert!(!(format.detect)(&jsonl));
    }
}
