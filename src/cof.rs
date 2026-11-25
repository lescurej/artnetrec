use std::fs;
use std::path::Path;

pub const FILE_SIZE: usize = 382;
const MAGIC: &[u8] = b"[Config]";
const TRIGGERS_AT: usize = 0x46;
const TRIGGER_SIZE: usize = 48;
const IDLE_FILE_AT: usize = 0x166;
const IDLE_MODE_AT: usize = 0x17C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TriggerMode {
    NoTr = 0,
    NcTr = 1,
    ToggleTr = 2,
    AutoTr = 3,
}

impl TriggerMode {
    pub fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::NoTr),
            1 => Some(Self::NcTr),
            2 => Some(Self::ToggleTr),
            3 => Some(Self::AutoTr),
            _ => None,
        }
    }

    pub const ALL: [TriggerMode; 4] = [
        TriggerMode::NoTr,
        TriggerMode::NcTr,
        TriggerMode::ToggleTr,
        TriggerMode::AutoTr,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TriggerStatus {
    Disable = 0,
    Enable = 1,
}

impl TriggerStatus {
    pub fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Disable),
            1 => Some(Self::Enable),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DmxPort {
    Single = 0,
    Htp = 1,
}

impl DmxPort {
    pub fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Single),
            1 => Some(Self::Htp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub name: String,
    pub index: u8,
    pub mode: TriggerMode,
    pub delay_secs: u8,
    pub status: TriggerStatus,
    pub show_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub firmware_block: [u8; 16],
    pub device_label: String,
    pub byte_3a: u8,
    pub display_a: u8,
    pub display_timer_steps: u8,
    pub display_b: u8,
    pub byte_3e: u8,
    pub byte_3f: u8,
    pub dmx_port: DmxPort,
    pub dmx_trigger_enabled: bool,
    pub dmx_trigger_address: u16,
    pub pad_44: [u8; 2],
    pub triggers: [Trigger; 6],
    pub idle_file: String,
    pub idle_mode: u16,
}

impl Default for Config {
    fn default() -> Self {
        let mut firmware_block = [0u8; 16];
        firmware_block[..7].copy_from_slice(b"YmK7003");
        firmware_block[7] = 0x0B;
        Self {
            firmware_block,
            device_label: "TR-512".into(),
            byte_3a: 0,
            display_a: 0,
            display_timer_steps: 6,
            display_b: 0,
            byte_3e: 1,
            byte_3f: 0,
            dmx_port: DmxPort::Single,
            dmx_trigger_enabled: false,
            dmx_trigger_address: 0,
            pad_44: [0, 0],
            triggers: std::array::from_fn(|index| Trigger {
                name: format!("Trigger{}", index + 1),
                index: index as u8,
                mode: TriggerMode::NoTr,
                delay_secs: 0,
                status: TriggerStatus::Disable,
                show_file: "No File".into(),
            }),
            idle_file: "No File".into(),
            idle_mode: 0,
        }
    }
}

pub fn load(path: &Path) -> Result<Config, String> {
    let data = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    parse(&data)
}

pub fn parse(data: &[u8]) -> Result<Config, String> {
    if data.len() != FILE_SIZE {
        return Err(format!("expected {FILE_SIZE} bytes, got {}", data.len()));
    }
    if &data[..MAGIC.len()] != MAGIC {
        return Err("not a COF configuration".into());
    }

    let mut firmware_block = [0u8; 16];
    firmware_block.copy_from_slice(&data[0x08..0x18]);

    let device_label = read_padded_ascii(&data[0x1A..0x3A]);
    let triggers = std::array::from_fn(|slot| parse_trigger(&data[TRIGGERS_AT + slot * TRIGGER_SIZE..]));

    Ok(Config {
        firmware_block,
        device_label,
        byte_3a: data[0x3A],
        display_a: data[0x3B],
        display_timer_steps: data[0x3C],
        display_b: data[0x3D],
        byte_3e: data[0x3E],
        byte_3f: data[0x3F],
        dmx_port: DmxPort::from_byte(data[0x40]).ok_or("invalid DMX port")?,
        dmx_trigger_enabled: data[0x41] != 0,
        dmx_trigger_address: u16::from_le_bytes([data[0x42], data[0x43]]),
        pad_44: [data[0x44], data[0x45]],
        triggers,
        idle_file: read_padded_ascii(&data[IDLE_FILE_AT..IDLE_FILE_AT + 22]),
        idle_mode: u16::from_le_bytes([data[IDLE_MODE_AT], data[IDLE_MODE_AT + 1]]),
    })
}

fn parse_trigger(data: &[u8]) -> Trigger {
    let mode = TriggerMode::from_byte(data[23]).unwrap_or(TriggerMode::NoTr);
    let status = TriggerStatus::from_byte(data[25]).unwrap_or(TriggerStatus::Disable);
    Trigger {
        name: read_padded_ascii(&data[..22]),
        index: data[22],
        mode,
        delay_secs: data[24],
        status,
        show_file: read_padded_ascii(&data[26..48]),
    }
}

pub fn to_bytes(config: &Config) -> [u8; FILE_SIZE] {
    let mut data = [0u8; FILE_SIZE];
    data[..MAGIC.len()].copy_from_slice(MAGIC);
    data[0x08..0x18].copy_from_slice(&config.firmware_block);
    data[0x18..0x1A].copy_from_slice(&374u16.to_le_bytes());
    write_padded_ascii(&mut data[0x1A..0x3A], &config.device_label, 32);
    data[0x3A] = config.byte_3a;
    data[0x3B] = config.display_a;
    data[0x3C] = config.display_timer_steps;
    data[0x3D] = config.display_b;
    data[0x3E] = config.byte_3e;
    data[0x3F] = config.byte_3f;
    data[0x40] = config.dmx_port as u8;
    data[0x41] = u8::from(config.dmx_trigger_enabled);
    data[0x42..0x44].copy_from_slice(&config.dmx_trigger_address.to_le_bytes());
    data[0x44..0x46].copy_from_slice(&config.pad_44);
    for (slot, trigger) in config.triggers.iter().enumerate() {
        encode_trigger(&mut data[TRIGGERS_AT + slot * TRIGGER_SIZE..], trigger);
    }
    write_padded_ascii(&mut data[IDLE_FILE_AT..IDLE_FILE_AT + 22], &config.idle_file, 22);
    data[IDLE_MODE_AT..IDLE_MODE_AT + 2].copy_from_slice(&config.idle_mode.to_le_bytes());
    data
}

fn encode_trigger(data: &mut [u8], trigger: &Trigger) {
    write_padded_ascii(&mut data[..22], &trigger.name, 22);
    data[22] = trigger.index;
    data[23] = trigger.mode as u8;
    data[24] = trigger.delay_secs;
    data[25] = trigger.status as u8;
    write_padded_ascii(&mut data[26..48], &trigger.show_file, 22);
}

pub fn write(path: &Path, config: &Config) -> Result<(), String> {
    fs::write(path, to_bytes(config))
        .map_err(|err| format!("cannot write {}: {err}", path.display()))
}

fn read_padded_ascii(data: &[u8]) -> String {
    let end = data.iter().position(|&byte| byte == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

fn write_padded_ascii(target: &mut [u8], text: &str, max_len: usize) {
    target.fill(0);
    let bytes = text.as_bytes();
    let len = bytes.len().min(max_len);
    target[..len].copy_from_slice(&bytes[..len]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/CONFIG")
    }

    #[test]
    fn round_trips_all_samples() {
        for entry in std::fs::read_dir(sample_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("COF") {
                continue;
            }
            let raw = std::fs::read(&path).unwrap();
            let config = parse(&raw).unwrap();
            let encoded = to_bytes(&config);
            assert_eq!(raw.as_slice(), encoded.as_slice(), "{}", path.display());
        }
    }
}