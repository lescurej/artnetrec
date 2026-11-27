use super::types::{BehaviorCatalog, BehaviorDef, BlendMode, ControlPoint};
use crate::show::edit::{self, CurveClear};
use crate::show::{Frame, Show};

pub fn regenerate(show: &mut Show, catalog: &BehaviorCatalog) -> Result<(), String> {
    if catalog.behaviors.is_empty() {
        return Ok(());
    }
    ensure_non_empty(show);
    for def in &catalog.behaviors {
        let channels = catalog.resolve_channels(def)?;
        apply_one(show, def, &channels)?;
    }
    show.sort_by_time();
    coalesce(show);
    Ok(())
}

fn apply_one(show: &mut Show, def: &BehaviorDef, channels: &[u16]) -> Result<(), String> {
    let points = ordered_points(&def.id, &def.points)?;
    let indices: Vec<usize> = channels.iter().map(|ch| usize::from(*ch - 1)).collect();
    let per_channel: Vec<Vec<(u32, u8)>> = indices
        .iter()
        .map(|&slot| {
            points
                .iter()
                .map(|point| {
                    let level = match def.blend {
                        BlendMode::Replace => point.level,
                        blend => blend.apply(show.value_at(slot, point.at_ms), point.level),
                    };
                    (point.at_ms, level)
                })
                .collect()
        })
        .collect();
    for (&slot, pairs) in indices.iter().zip(per_channel) {
        edit::write_channel_points(show, slot, &pairs, CurveClear::Span);
    }
    Ok(())
}

fn ordered_points(id: &str, points: &[ControlPoint]) -> Result<Vec<ControlPoint>, String> {
    if points.is_empty() {
        return Err(format!("behavior {id} has no points"));
    }
    let mut ordered = points.to_vec();
    ordered.sort_by_key(|point| point.at_ms);
    let mut out = Vec::with_capacity(ordered.len());
    for point in ordered {
        if out
            .last()
            .is_some_and(|last: &ControlPoint| last.at_ms == point.at_ms)
        {
            out.pop();
        }
        out.push(point);
    }
    Ok(out)
}

fn ensure_non_empty(show: &mut Show) {
    if show.frames.is_empty() {
        show.frames.push(Frame::blank(0));
    }
}

fn coalesce(show: &mut Show) {
    if show.frames.len() < 2 {
        return;
    }
    let mut merged: Vec<Frame> = Vec::with_capacity(show.frames.len());
    for frame in show.frames.drain(..) {
        if let Some(last) = merged.last() {
            if last.slots == frame.slots && last.keys == frame.keys {
                continue;
            }
        }
        merged.push(frame);
    }
    show.frames = merged;
}
