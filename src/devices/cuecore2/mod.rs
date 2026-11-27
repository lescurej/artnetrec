pub mod track;
pub mod xml;

use super::{Device, ShowFormat, Status};

pub const DEVICE: Device = Device {
    name: "Visual Productions CueCore2",
    code: None,
    status: Status::Supported,
    detail: "Import one track from a vManager backup. Export writes that track only.",
    show: Some(ShowFormat {
        label: "CueCore2",
        extension: "xml",
        extensions: &["xml", "XML"],
        detect: xml::is_file,
        read: xml::parse,
        write: xml::to_bytes,
    }),
    config_extensions: &[],
};
