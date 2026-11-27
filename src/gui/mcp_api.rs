use super::{App, LoadedFile};
use crate::behavior::types::GroupMembers;
use crate::behavior::{self, BehaviorCatalog};
use crate::session::ApiRequest;
use crate::show::edit;
use crate::show::Show;
use serde::Serialize;

#[derive(Serialize)]
struct ChannelSummary {
    channel: u16,
    min: u8,
    max: u8,
    keyframe_changes: usize,
}

pub fn handle_api(app: &mut App, request: ApiRequest) -> Result<String, String> {
    match request {
        ApiRequest::ShowStatus => {
            let mut value = serde_json::json!({
                "show_open": matches!(app.document, Some(LoadedFile::Show { .. })),
                "path": app.path.as_ref().map(|p| p.display().to_string()),
                "dirty": app.dirty,
                "mcp_port": app.mcp_port,
            });
            if let Some(LoadedFile::Show { show, view, .. }) = &app.document {
                value["end_ms"] = show.end_ms().into();
                value["frame_count"] = show.frames.len().into();
                value["playing"] = view.playing.into();
            }
            Ok(value.to_string())
        }
        ApiRequest::DescribeShow {
            channels,
            sample_points,
        } => match &app.document {
            Some(LoadedFile::Show { show, view, .. }) => {
                let active = show.active_channels();
                let focus: Vec<u16> = channels.unwrap_or(active.clone());
                let sample_points = sample_points.unwrap_or(12).clamp(2, 64);
                let summaries = channel_summaries(show, &focus);
                let mut curves = serde_json::Map::new();
                for channel in &focus {
                    if let Ok(points) = edit::sample_track(show, *channel, sample_points) {
                        curves.insert(
                            channel.to_string(),
                            serde_json::to_value(points).unwrap_or_default(),
                        );
                    }
                }
                let value = serde_json::json!({
                    "active_channels": active,
                    "channels": summaries,
                    "track_samples": curves,
                    "transfers": view.curves.export_json(),
                    "master": view.master,
                    "master_gamma": view.curves.master_gamma(),
                    "behaviors": app.behavior_catalog.behaviors,
                    "groups": app.behavior_catalog.groups,
                });
                Ok(value.to_string())
            }
            _ => Err("open a show in the editor".into()),
        },
        ApiRequest::ListTrackKeyframes { channel } => match &app.document {
            Some(LoadedFile::Show { show, .. }) => {
                let points = edit::track_keyframes(show, channel)?;
                Ok(serde_json::to_string(&points).map_err(|err| err.to_string())?)
            }
            _ => Err("open a show in the editor".into()),
        },
        ApiRequest::DescribeTrack {
            channel,
            max_points,
        } => match &app.document {
            Some(LoadedFile::Show { show, view, .. }) => {
                let max_points = max_points.unwrap_or(24);
                let keyframes = edit::track_keyframes(show, channel)?;
                let samples = edit::sample_track(show, channel, max_points)?;
                Ok(serde_json::json!({
                    "channel": channel,
                    "keyframes": keyframes,
                    "samples": samples,
                    "transfer": view.curves.channel_json(channel),
                })
                .to_string())
            }
            _ => Err("open a show in the editor".into()),
        },
        ApiRequest::SampleChannels { at_ms, channels } => match &app.document {
            Some(LoadedFile::Show { show, .. }) => {
                let mut levels = serde_json::Map::new();
                for ch in channels {
                    if !(1..=512).contains(&ch) {
                        return Err(format!("channel {ch} is outside 1..512"));
                    }
                    let level = show.value_at(usize::from(ch - 1), at_ms);
                    levels.insert(ch.to_string(), level.into());
                }
                Ok(serde_json::json!({ "at_ms": at_ms, "levels": levels }).to_string())
            }
            _ => Err("open a show in the editor".into()),
        },
        ApiRequest::ListGroups => {
            Ok(serde_json::to_string(&app.behavior_catalog.groups)
                .map_err(|err| err.to_string())?)
        }
        ApiRequest::SetGroup { name, members } => {
            validate_group(&name, &members)?;
            app.behavior_catalog.groups.insert(name.clone(), members);
            app.dirty = true;
            save_sidecar_if_possible(app)?;
            Ok(serde_json::json!({ "ok": true, "name": name }).to_string())
        }
        ApiRequest::DeleteGroup { name } => {
            if app.behavior_catalog.groups.remove(&name).is_none() {
                return Err(format!("unknown group \"{name}\""));
            }
            app.dirty = true;
            save_sidecar_if_possible(app)?;
            Ok(serde_json::json!({ "ok": true, "name": name }).to_string())
        }
        ApiRequest::ListBehaviors => Ok(serde_json::to_string(&app.behavior_catalog.behaviors)
            .map_err(|err| err.to_string())?),
        ApiRequest::ApplyBehavior { def } => {
            let touched = app
                .behavior_catalog
                .resolve_channels(&def)
                .map_err(|err| err.to_string())?;
            if let Some(index) = app
                .behavior_catalog
                .behaviors
                .iter()
                .position(|entry| entry.id == def.id)
            {
                app.behavior_catalog.behaviors[index] = def.clone();
            } else {
                app.behavior_catalog.behaviors.push(def.clone());
            }
            let edited = match &mut app.document {
                Some(LoadedFile::Show { show, view, undo }) => {
                    let before = show.frames.clone();
                    behavior::regenerate(show, &app.behavior_catalog)?;
                    if before != show.frames {
                        undo.push(vec![super::undo::UndoOp::ReplaceFrames {
                            before,
                            after: show.frames.clone(),
                        }]);
                    }
                    super::ncod_view::NcodViewState::refresh_after_show_mutation(show, view);
                    let response = serde_json::json!({
                        "ok": true,
                        "id": def.id,
                        "frame_count": show.frames.len(),
                        "end_ms": show.end_ms(),
                    })
                    .to_string();
                    app.dirty = true;
                    Ok(response)
                }
                _ => Err("open a show in the editor".into()),
            };
            if edited.is_ok() {
                note_mcp_channels(app, &touched);
            }
            edited.map(|response| {
                let _ = save_sidecar_if_possible(app);
                response
            })
        }
        ApiRequest::RemoveBehavior { id } => {
            let touched = app
                .behavior_catalog
                .behaviors
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|def| app.behavior_catalog.resolve_channels(def).ok())
                .unwrap_or_default();
            let edited = match &mut app.document {
                Some(LoadedFile::Show {
                    show, view, undo, ..
                }) => {
                    let removed = app
                        .behavior_catalog
                        .behaviors
                        .iter()
                        .position(|entry| entry.id == id)
                        .map(|index| app.behavior_catalog.behaviors.remove(index));
                    if removed.is_none() {
                        return Err(format!("unknown behavior \"{id}\""));
                    }
                    let before = show.frames.clone();
                    behavior::regenerate(show, &app.behavior_catalog)?;
                    if before != show.frames {
                        undo.push(vec![super::undo::UndoOp::ReplaceFrames {
                            before,
                            after: show.frames.clone(),
                        }]);
                    }
                    super::ncod_view::NcodViewState::refresh_after_show_mutation(show, view);
                    let response = serde_json::json!({
                        "ok": true,
                        "id": id,
                        "frame_count": show.frames.len(),
                    })
                    .to_string();
                    app.dirty = true;
                    Ok(response)
                }
                _ => Err("open a show in the editor".into()),
            };
            if edited.is_ok() {
                note_mcp_channels(app, &touched);
            }
            edited.map(|response| {
                let _ = save_sidecar_if_possible(app);
                response
            })
        }
        ApiRequest::RegenerateShow => {
            let touched = behavior_channels(app);
            let edited = match &mut app.document {
                Some(LoadedFile::Show {
                    show, view, undo, ..
                }) => {
                    let before = show.frames.clone();
                    behavior::regenerate(show, &app.behavior_catalog)?;
                    if before != show.frames {
                        undo.push(vec![super::undo::UndoOp::ReplaceFrames {
                            before,
                            after: show.frames.clone(),
                        }]);
                    }
                    super::ncod_view::NcodViewState::refresh_after_show_mutation(show, view);
                    let response = serde_json::json!({
                        "ok": true,
                        "frame_count": show.frames.len(),
                        "end_ms": show.end_ms(),
                    })
                    .to_string();
                    app.dirty = true;
                    Ok(response)
                }
                _ => Err("open a show in the editor".into()),
            };
            if edited.is_ok() {
                note_mcp_channels(app, &touched);
            }
            edited.map(|response| {
                let _ = save_sidecar_if_possible(app);
                response
            })
        }
        ApiRequest::SaveShow => {
            if !app.dirty && app.path.is_some() {
                return Ok(
                    serde_json::json!({ "ok": true, "saved": false, "reason": "unchanged" })
                        .to_string(),
                );
            }
            let path = app
                .path
                .clone()
                .ok_or_else(|| "no file path; save from the editor first".to_string())?;
            if !app.save_to(path) {
                return Err(app.error.clone().unwrap_or_else(|| "save failed".into()));
            }
            save_sidecar_if_possible(app)?;
            Ok(serde_json::json!({ "ok": true, "saved": true }).to_string())
        }
        ApiRequest::SetTransfer {
            channel,
            law,
            min_on,
        } => {
            let response = match &app.document {
                Some(LoadedFile::Show { view, .. }) => {
                    view.curves.set(channel, law)?;
                    if let Some(min_on) = min_on {
                        view.curves.set_min_on(channel, min_on)?;
                    }
                    Ok(serde_json::json!({
                        "ok": true,
                        "channel": channel,
                        "transfer": view.curves.channel_json(channel),
                    })
                    .to_string())
                }
                _ => Err("open a show in the editor".into()),
            };
            if response.is_ok() {
                app.dirty = true;
            }
            response
        }
        ApiRequest::SetMasterGamma { gamma } => {
            let response = match &app.document {
                Some(LoadedFile::Show { view, .. }) => {
                    view.curves.set_master_gamma(gamma)?;
                    Ok(serde_json::json!({
                        "ok": true,
                        "master_gamma": view.curves.master_gamma(),
                    })
                    .to_string())
                }
                _ => Err("open a show in the editor".into()),
            };
            if response.is_ok() {
                app.dirty = true;
            }
            response
        }
    }
}

fn note_mcp_channels(app: &mut App, channels: &[u16]) {
    let Some(LoadedFile::Show { view, .. }) = &mut app.document else {
        return;
    };
    view.mark_mcp_channels(channels.iter().copied(), !app.mcp_write_batch);
    app.mcp_write_batch = true;
}

fn behavior_channels(app: &App) -> Vec<u16> {
    let mut channels = Vec::new();
    for def in &app.behavior_catalog.behaviors {
        let Ok(list) = app.behavior_catalog.resolve_channels(def) else {
            continue;
        };
        for channel in list {
            if !channels.contains(&channel) {
                channels.push(channel);
            }
        }
    }
    channels
}

fn validate_group(name: &str, members: &GroupMembers) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("group name is empty".into());
    }
    crate::behavior::types::expand_group(members).map(|_| ())
}

fn channel_summaries(show: &Show, channels: &[u16]) -> Vec<ChannelSummary> {
    channels
        .iter()
        .map(|&channel| {
            let slot = usize::from(channel - 1);
            let mut min = 255u8;
            let mut max = 0u8;
            let mut changes = 0usize;
            let mut previous: Option<u8> = None;
            for frame in &show.frames {
                let level = frame.slots[slot];
                if level != 0 {
                    min = min.min(level);
                    max = max.max(level);
                }
                if previous != Some(level) {
                    changes += 1;
                    previous = Some(level);
                }
            }
            if max == 0 {
                min = 0;
            }
            ChannelSummary {
                channel,
                min,
                max,
                keyframe_changes: changes,
            }
        })
        .collect()
}

pub fn save_sidecar_if_possible(app: &App) -> Result<(), String> {
    let Some(path) = &app.path else {
        return Ok(());
    };
    let sidecar = behavior::store::sidecar_path(path);
    behavior::store::save(&sidecar, &app.behavior_catalog)?;
    Ok(())
}

pub fn load_sidecar(path: &std::path::Path) -> BehaviorCatalog {
    let sidecar = behavior::store::sidecar_path(path);
    behavior::store::load(&sidecar).unwrap_or_else(|err| {
        eprintln!("behaviors: {err}");
        BehaviorCatalog::new()
    })
}
