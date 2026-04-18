pub mod pb {
    tonic::include_proto!("macp.v1");
}

pub mod decision_pb {
    tonic::include_proto!("macp.modes.decision.v1");
}

pub mod proposal_pb {
    tonic::include_proto!("macp.modes.proposal.v1");
}

pub mod task_pb {
    tonic::include_proto!("macp.modes.task.v1");
}

pub mod handoff_pb {
    tonic::include_proto!("macp.modes.handoff.v1");
}

pub mod quorum_pb {
    tonic::include_proto!("macp.modes.quorum.v1");
}

pub mod error;
pub mod log_store;
pub mod metrics;
pub mod mode;
pub mod mode_registry;
pub mod policy;
pub mod registry;
pub mod replay;
pub mod runtime;
pub mod session;
pub mod storage;
pub mod stream_bus;

pub mod auth;
pub mod extensions;
pub mod security;
