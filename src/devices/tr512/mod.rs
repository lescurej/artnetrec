pub mod cof;
pub mod ncod;

use super::{Device, ShowFormat, Status};

pub const DEVICE: Device = Device {
    name: "Showtec TR-512",
    code: Some("50825"),
    status: Status::Supported,
    detail: "Import and export .NCOD shows and .COF configurations.",
    show: Some(ShowFormat {
        label: "Showtec TR-512",
        extension: "ncod",
        extensions: &["ncod", "NCOD"],
        detect: ncod::is_file,
        read: ncod::parse,
        write: ncod::to_bytes,
    }),
    config_extensions: &["cof", "COF"],
};

pub fn open_config(extension: Option<&str>, bytes: &[u8]) -> Option<Result<cof::Config, String>> {
    let by_extension = extension.is_some_and(|ext| {
        DEVICE
            .config_extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    });
    if by_extension || cof::is_file(bytes) {
        Some(cof::parse(bytes))
    } else {
        None
    }
}
