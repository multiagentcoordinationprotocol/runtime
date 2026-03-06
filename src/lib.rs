pub mod pb {
    tonic::include_proto!("macp.v1");
}

pub mod decision_pb {
    tonic::include_proto!("macp.modes.decision.v1");
}

pub mod error;
pub mod log_store;
pub mod mode;
pub mod registry;
pub mod runtime;
pub mod session;
