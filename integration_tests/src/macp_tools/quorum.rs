use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

use super::decision::MacpToolError;
use super::SharedClient;
use crate::helpers;

#[derive(Serialize)]
pub struct ToolResult {
    pub ok: bool,
    pub session_state: i32,
}

#[derive(Clone)]
pub struct ApprovalRequestTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct ApprovalRequestArgs {
    pub session_id: String,
    pub request_id: String,
    pub action: String,
    pub summary: String,
    pub required_approvals: u32,
}

impl Tool for ApprovalRequestTool {
    const NAME: &'static str = "macp_approval_request";
    type Error = MacpToolError;
    type Args = ApprovalRequestArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_approval_request",
            "description": "Submit an approval request in a MACP Quorum session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "request_id": { "type": "string" },
                    "action": { "type": "string" },
                    "summary": { "type": "string" },
                    "required_approvals": { "type": "integer" }
                },
                "required": ["session_id", "request_id", "action", "summary", "required_approvals"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::approval_request_payload(
            &args.request_id,
            &args.action,
            &args.summary,
            args.required_approvals,
        );
        let env = helpers::envelope(
            helpers::MODE_QUORUM,
            "ApprovalRequest",
            &helpers::new_message_id(),
            &args.session_id,
            &self.agent_id,
            payload,
        );
        let mut client = self.client.lock().await;
        let ack = helpers::send_as(&mut client, &self.agent_id, env)
            .await
            .map_err(|e| MacpToolError(e.to_string()))?;
        Ok(ToolResult {
            ok: ack.ok,
            session_state: ack.session_state,
        })
    }
}

#[derive(Clone)]
pub struct ApproveTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct ApproveArgs {
    pub session_id: String,
    pub request_id: String,
    pub reason: String,
}

impl Tool for ApproveTool {
    const NAME: &'static str = "macp_approve";
    type Error = MacpToolError;
    type Args = ApproveArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_approve",
            "description": "Approve a request in a MACP Quorum session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "request_id": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["session_id", "request_id", "reason"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::approve_payload(&args.request_id, &args.reason);
        let env = helpers::envelope(
            helpers::MODE_QUORUM,
            "Approve",
            &helpers::new_message_id(),
            &args.session_id,
            &self.agent_id,
            payload,
        );
        let mut client = self.client.lock().await;
        let ack = helpers::send_as(&mut client, &self.agent_id, env)
            .await
            .map_err(|e| MacpToolError(e.to_string()))?;
        Ok(ToolResult {
            ok: ack.ok,
            session_state: ack.session_state,
        })
    }
}
