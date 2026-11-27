use crate::behavior::types::{BehaviorDef, GroupMembers};
use crossbeam_channel::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct SessionFacade {
    job_tx: Sender<ApiJob>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct ApiJob {
    pub request: ApiRequest,
    pub respond: Sender<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum ApiRequest {
    ShowStatus,
    DescribeShow {
        channels: Option<Vec<u16>>,
        sample_points: Option<usize>,
    },
    ListTrackKeyframes {
        channel: u16,
    },
    DescribeTrack {
        channel: u16,
        max_points: Option<usize>,
    },
    SampleChannels {
        at_ms: u32,
        channels: Vec<u16>,
    },
    ListGroups,
    SetGroup {
        name: String,
        members: GroupMembers,
    },
    DeleteGroup {
        name: String,
    },
    ListBehaviors,
    ApplyBehavior {
        def: BehaviorDef,
    },
    RemoveBehavior {
        id: String,
    },
    RegenerateShow,
    SaveShow,
    SetTransfer {
        channel: u16,
        law: crate::curve::TransferLaw,
        min_on: Option<u8>,
    },
    SetMasterGamma {
        gamma: f32,
    },
}

pub fn pair() -> (SessionFacade, Receiver<ApiJob>) {
    let (job_tx, job_rx) = crossbeam_channel::unbounded();
    (SessionFacade { job_tx, wake: None }, job_rx)
}

impl std::fmt::Debug for SessionFacade {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionFacade")
    }
}

impl SessionFacade {
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    pub fn call(&self, request: ApiRequest) -> Result<String, String> {
        let (respond, reply) = crossbeam_channel::bounded(1);
        self.job_tx
            .send(ApiJob { request, respond })
            .map_err(|_| "editor is not accepting MCP commands".to_string())?;
        if let Some(wake) = &self.wake {
            wake();
        }
        reply
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| "editor did not respond in time".to_string())?
    }
}
