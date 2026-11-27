use crate::behavior::types::{BehaviorDef, GroupMembers};
use crate::session::{ApiRequest, SessionFacade};
use rmcp::{
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct SetGroupRequest {
    pub name: String,
    pub members: GroupMembers,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteGroupRequest {
    pub name: String,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct ApplyBehaviorRequest {
    pub behavior: BehaviorDef,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct RemoveBehaviorRequest {
    pub id: String,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct SampleChannelsRequest {
    pub at_ms: u32,
    pub channels: Vec<u16>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct DescribeShowRequest {
    #[serde(default)]
    pub channels: Option<Vec<u16>>,
    #[serde(default)]
    pub sample_points: Option<usize>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct ChannelRequest {
    pub channel: u16,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct SetTransferRequest {
    #[schemars(description = "DMX channel 1..512")]
    pub channel: u16,
    #[schemars(
        description = "Transmission law applied when this channel is sent on Art-Net. The show file is unchanged. gamma 1 is linear, 2 is square, 0.5 is inverse square, 0.3 matches a tungsten bulb."
    )]
    pub law: crate::curve::TransferLaw,
    #[serde(default)]
    #[schemars(
        description = "DMX level where this lamp strikes. Omit to leave it unchanged. 0 stays off. Any programmed level above 0 starts at this value, then the curve runs up to 255."
    )]
    pub min_on: Option<u8>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct SetMasterGammaRequest {
    #[schemars(
        description = "Power curve of the master fader. 1 is linear. Below 1 rises early so a tungsten rig dims evenly. 0 and 255 stay exact."
    )]
    pub gamma: f32,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct DescribeTrackRequest {
    pub channel: u16,
    #[serde(default)]
    pub max_points: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct DmxHandler {
    facade: SessionFacade,
    port: u16,
    tool_router: ToolRouter<Self>,
}

impl DmxHandler {
    pub fn new(facade: SessionFacade, port: u16) -> Self {
        Self {
            facade,
            port,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl DmxHandler {
    #[tool(description = "Path, frame count, end time, dirty flag, playback, MCP port")]
    fn show_status(&self) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::ShowStatus)?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Show summary, behaviors, and downsampled track curves for active or listed channels"
    )]
    fn describe_show(
        &self,
        Parameters(req): Parameters<DescribeShowRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::DescribeShow {
            channels: req.channels,
            sample_points: req.sample_points,
        })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Control points on one channel. Values between them are interpolated, so the list is not one entry per frame."
    )]
    fn list_track_keyframes(
        &self,
        Parameters(req): Parameters<ChannelRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::ListTrackKeyframes {
            channel: req.channel,
        })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Control points on one channel, plus a downsampled view of the interpolated curve."
    )]
    fn describe_track(
        &self,
        Parameters(req): Parameters<DescribeTrackRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::DescribeTrack {
            channel: req.channel,
            max_points: req.max_points,
        })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Interpolated DMX levels at one time. Use this to check a curve. It does not add keyframes."
    )]
    fn sample_channels(
        &self,
        Parameters(req): Parameters<SampleChannelsRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::SampleChannels {
            at_ms: req.at_ms,
            channels: req.channels,
        })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "List named channel groups")]
    fn list_groups(&self) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::ListGroups)?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Create or replace a named channel group")]
    fn set_group(
        &self,
        Parameters(req): Parameters<SetGroupRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::SetGroup {
            name: req.name,
            members: req.members,
        })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Delete a named channel group")]
    fn delete_group(
        &self,
        Parameters(req): Parameters<DeleteGroupRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self
            .facade
            .call(ApiRequest::DeleteGroup { name: req.name })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "List behavior definitions in the sidecar")]
    fn list_behaviors(&self) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::ListBehaviors)?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Add or replace a behavior with the control points given. Writes those points on its channels. Playback linearly interpolates between them."
    )]
    fn apply_behavior(
        &self,
        Parameters(req): Parameters<ApplyBehaviorRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self
            .facade
            .call(ApiRequest::ApplyBehavior { def: req.behavior })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Remove a behavior definition and regenerate")]
    fn remove_behavior(
        &self,
        Parameters(req): Parameters<RemoveBehaviorRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self
            .facade
            .call(ApiRequest::RemoveBehavior { id: req.id })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Re-run all behaviors on the open show")]
    fn regenerate_show(&self) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::RegenerateShow)?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Set the transmission curve for one channel. The programmed show stays unchanged. Art-Net output maps the level through this law because lamps are not linear. gamma 0.3 matches a tungsten bulb, 2 is square, 0.5 is inverse square, 1 is linear. min_on is the DMX level where the lamp strikes; 0 stays off. Saved beside the show as .curves.json."
    )]
    fn set_transfer_curve(
        &self,
        Parameters(req): Parameters<SetTransferRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::SetTransfer {
            channel: req.channel,
            law: req.law,
            min_on: req.min_on,
        })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Set the gamma of the master fader. 1 is linear. Below 1 the master rises early so the rig dims evenly. 0 and 255 stay exact. Saved in the curves sidecar."
    )]
    fn set_master_gamma(
        &self,
        Parameters(req): Parameters<SetMasterGammaRequest>,
    ) -> Result<Json<serde_json::Value>, String> {
        let raw = self
            .facade
            .call(ApiRequest::SetMasterGamma { gamma: req.gamma })?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Write the open show to disk. Do not call this. Curve and behavior edits stay in memory. Only call it when the user's latest message explicitly asks to save or write the file."
    )]
    fn save_show(&self) -> Result<Json<serde_json::Value>, String> {
        let raw = self.facade.call(ApiRequest::SaveShow)?;
        serde_json::from_str(&raw)
            .map(Json)
            .map_err(|err| err.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DmxHandler {
    fn get_info(&self) -> ServerConfig {
        let instructions = format!(
            "DMX Recorder MCP on 127.0.0.1:{}. Never call save_show. \
             Open a .jsonl show first. To edit the show, overwrite that file. show_status returns its path. \
             Write a temp file in the same directory, then rename it onto the show path. \
             The editor reloads the file. \
             The file is the whole show: first line {{\"dmx-recorder\":2}}, \
             then one look per line {{\"at_ms\":<ms>,\"channels\":{{\"<channel>\":<level>}}}}. \
             Copy master, master_gamma, and transmission from the existing header when you rewrite the file. \
             A missing channel is 0. Times do not go backwards. Levels are 0..255. Channels are 1..512. \
             Playback linearly interpolates between looks. Outside the first and last look, the level holds. \
             describe_show, describe_track, list_track_keyframes, and sample_channels only read. \
             apply_behavior stores a behavior. regenerate_show writes stored behaviors again inside their time spans. \
             set_transfer_curve and set_master_gamma are stored in the jsonl header and applied on Art-Net. They do not change programmed levels. gamma 0.3 compensates a tungsten bulb.",
            self.port
        );
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
    }
}
