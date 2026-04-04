use crate::common;
use macp_integration_tests::helpers;

#[tokio::test]
async fn initialize_returns_protocol_version() {
    let mut client = common::grpc_client().await;
    let resp = helpers::initialize(&mut client).await.unwrap();
    assert_eq!(resp.selected_protocol_version, "1.0");
}

#[tokio::test]
async fn initialize_returns_runtime_info() {
    let mut client = common::grpc_client().await;
    let resp = helpers::initialize(&mut client).await.unwrap();
    let info = resp.runtime_info.expect("runtime_info present");
    assert!(!info.name.is_empty());
    assert!(!info.version.is_empty());
}

#[tokio::test]
async fn list_modes_returns_five_standard_modes() {
    let mut client = common::grpc_client().await;
    let resp = helpers::list_modes(&mut client).await.unwrap();
    let mode_ids: Vec<&str> = resp.modes.iter().map(|m| m.mode.as_str()).collect();
    assert!(mode_ids.contains(&"macp.mode.decision.v1"));
    assert!(mode_ids.contains(&"macp.mode.proposal.v1"));
    assert!(mode_ids.contains(&"macp.mode.task.v1"));
    assert!(mode_ids.contains(&"macp.mode.handoff.v1"));
    assert!(mode_ids.contains(&"macp.mode.quorum.v1"));
    assert_eq!(resp.modes.len(), 5);
}
