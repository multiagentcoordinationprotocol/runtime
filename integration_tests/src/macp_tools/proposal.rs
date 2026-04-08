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
pub struct SubmitProposalTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct SubmitProposalArgs {
    pub session_id: String,
    pub proposal_id: String,
    pub title: String,
    pub summary: String,
}

impl Tool for SubmitProposalTool {
    const NAME: &'static str = "macp_submit_proposal";
    type Error = MacpToolError;
    type Args = SubmitProposalArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_submit_proposal",
            "description": "Submit a proposal in a MACP Proposal negotiation session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "proposal_id": { "type": "string" },
                    "title": { "type": "string" },
                    "summary": { "type": "string" }
                },
                "required": ["session_id", "proposal_id", "title", "summary"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::proposal_mode_payload(&args.proposal_id, &args.title, &args.summary);
        let env = helpers::envelope(
            helpers::MODE_PROPOSAL,
            "Proposal",
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
pub struct AcceptProposalTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct AcceptProposalArgs {
    pub session_id: String,
    pub proposal_id: String,
    pub reason: String,
}

impl Tool for AcceptProposalTool {
    const NAME: &'static str = "macp_accept_proposal";
    type Error = MacpToolError;
    type Args = AcceptProposalArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_accept_proposal",
            "description": "Accept a proposal in a MACP Proposal negotiation session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "proposal_id": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["session_id", "proposal_id", "reason"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::accept_proposal_payload(&args.proposal_id, &args.reason);
        let env = helpers::envelope(
            helpers::MODE_PROPOSAL,
            "Accept",
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
