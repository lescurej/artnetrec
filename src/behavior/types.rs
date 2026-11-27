use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Replace,
    Max,
    Add,
}

impl BlendMode {
    pub fn apply(self, current: u8, incoming: u8) -> u8 {
        match self {
            Self::Replace => incoming,
            Self::Max => current.max(incoming),
            Self::Add => current.saturating_add(incoming),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupMembers {
    Channels(Vec<u16>),
    Range { from: u16, to: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ControlPoint {
    #[schemars(description = "Absolute time in milliseconds")]
    pub at_ms: u32,
    pub level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BehaviorDef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[schemars(description = "Control points. Playback linearly interpolates between them.")]
    pub points: Vec<ControlPoint>,
    #[serde(default)]
    pub blend: BlendMode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BehaviorCatalog {
    pub version: u32,
    pub groups: HashMap<String, GroupMembers>,
    pub behaviors: Vec<BehaviorDef>,
}

impl BehaviorCatalog {
    pub const VERSION: u32 = 1;

    pub fn new() -> Self {
        Self {
            version: Self::VERSION,
            groups: HashMap::new(),
            behaviors: Vec::new(),
        }
    }

    pub fn resolve_channels(&self, def: &BehaviorDef) -> Result<Vec<u16>, String> {
        if let Some(channels) = &def.channels {
            if def.group.is_some() {
                return Err(format!(
                    "behavior {}: use channels or group, not both",
                    def.id
                ));
            }
            return normalize_channels(channels);
        }
        let name = def
            .group
            .as_ref()
            .ok_or_else(|| format!("behavior {}: missing channels or group", def.id))?;
        let members = self
            .groups
            .get(name)
            .ok_or_else(|| format!("unknown group \"{name}\""))?;
        expand_group(members)
    }
}

pub fn normalize_channels(channels: &[u16]) -> Result<Vec<u16>, String> {
    if channels.is_empty() {
        return Err("channel list is empty".into());
    }
    let mut out = Vec::new();
    for &ch in channels {
        if !(1..=512).contains(&ch) {
            return Err(format!("channel {ch} is outside 1..512"));
        }
        if !out.contains(&ch) {
            out.push(ch);
        }
    }
    out.sort_unstable();
    Ok(out)
}

pub fn expand_group(members: &GroupMembers) -> Result<Vec<u16>, String> {
    match members {
        GroupMembers::Channels(list) => normalize_channels(list),
        GroupMembers::Range { from, to } => {
            if from > to {
                return Err(format!("invalid range {from}..{to}"));
            }
            if *from < 1 || *to > 512 {
                return Err("range must stay within 1..512".into());
            }
            Ok((*from..=*to).collect())
        }
    }
}
