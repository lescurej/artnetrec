mod apply;
pub mod store;
pub mod types;

pub use apply::regenerate;
pub use types::{BehaviorCatalog, BehaviorDef, BlendMode, ControlPoint, GroupMembers};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{Frame, Show};
    use types::{BehaviorDef, BlendMode, ControlPoint};

    fn point(at_ms: u32, level: u8) -> ControlPoint {
        ControlPoint { at_ms, level }
    }

    #[test]
    fn two_points_reach_the_end_level() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512])],
        };
        let mut catalog = BehaviorCatalog::new();
        catalog.behaviors.push(BehaviorDef {
            id: "r1".into(),
            channels: Some(vec![1]),
            group: None,
            points: vec![point(0, 0), point(1000, 255)],
            blend: BlendMode::Replace,
        });
        regenerate(&mut show, &catalog).unwrap();
        let last = show.frames.last().unwrap();
        assert_eq!(last.slots[0], 255);
    }

    #[test]
    fn level_between_two_points_is_interpolated() {
        let mut frames = Vec::new();
        for step in 0..=10 {
            let mut slots = [0u8; 512];
            slots[1] = 10;
            frames.push(Frame::full(step * 20, slots));
        }
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames,
        };
        let mut catalog = BehaviorCatalog::new();
        catalog.behaviors.push(BehaviorDef {
            id: "rise".into(),
            channels: Some(vec![1]),
            group: None,
            points: vec![point(0, 0), point(200, 200)],
            blend: BlendMode::Replace,
        });
        regenerate(&mut show, &catalog).unwrap();
        assert_eq!(show.channel_keys(0).len(), 2);
        for frame in &show.frames {
            let progress = frame.at_ms as f32 / 200.0;
            let expected = (200.0 * progress).round() as u8;
            assert_eq!(
                show.value_at(0, frame.at_ms),
                expected,
                "at {}",
                frame.at_ms
            );
            assert_eq!(show.value_at(1, frame.at_ms), 10, "at {}", frame.at_ms);
        }
    }

    #[test]
    fn same_points_apply_to_every_channel_in_the_group() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512])],
        };
        let mut catalog = BehaviorCatalog::new();
        catalog
            .groups
            .insert("rgb".into(), GroupMembers::Channels(vec![1, 2, 3]));
        catalog.behaviors.push(BehaviorDef {
            id: "c1".into(),
            channels: None,
            group: Some("rgb".into()),
            points: vec![point(0, 0), point(300, 200)],
            blend: BlendMode::Replace,
        });
        regenerate(&mut show, &catalog).unwrap();
        assert_eq!(show.value_at(0, 300), 200);
        assert_eq!(show.value_at(1, 300), 200);
        assert_eq!(show.value_at(2, 300), 200);
    }

    #[test]
    fn timestamps_stay_monotonic() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512])],
        };
        let mut catalog = BehaviorCatalog::new();
        catalog.behaviors.push(BehaviorDef {
            id: "h".into(),
            channels: Some(vec![5]),
            group: None,
            points: vec![point(100, 128), point(300, 128)],
            blend: BlendMode::Max,
        });
        regenerate(&mut show, &catalog).unwrap();
        assert!(show.timestamps_monotonic());
        assert_eq!(show.value_at(4, 200), 128);
    }

    #[test]
    fn irregular_points_are_stored_as_given() {
        let mut show = Show {
            output: crate::curve::ShowOutput::default(),
            frames: vec![Frame::full(0, [0; 512])],
        };
        let mut catalog = BehaviorCatalog::new();
        catalog.behaviors.push(BehaviorDef {
            id: "shape".into(),
            channels: Some(vec![1]),
            group: None,
            points: vec![point(0, 10), point(40, 200), point(50, 30), point(90, 255)],
            blend: BlendMode::Replace,
        });
        regenerate(&mut show, &catalog).unwrap();
        let keys = show.channel_keys(0);
        assert_eq!(keys, vec![(0, 10), (40, 200), (50, 30), (90, 255)]);
    }
}
