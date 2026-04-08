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

// ── TaskRequestTool ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TaskRequestTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct TaskRequestArgs {
    pub session_id: String,
    pub task_id: String,
    pub title: String,
    pub instructions: String,
    pub assignee: String,
}

impl Tool for TaskRequestTool {
    const NAME: &'static str = "macp_task_request";
    type Error = MacpToolError;
    type Args = TaskRequestArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_task_request",
            "description": "Create a task request in a MACP Task session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "task_id": { "type": "string" },
                    "title": { "type": "string" },
                    "instructions": { "type": "string" },
                    "assignee": { "type": "string" }
                },
                "required": ["session_id", "task_id", "title", "instructions", "assignee"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::task_request_payload(
            &args.task_id,
            &args.title,
            &args.instructions,
            &args.assignee,
        );
        let env = helpers::envelope(
            helpers::MODE_TASK,
            "TaskRequest",
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

// ── TaskAcceptTool ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TaskAcceptTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct TaskAcceptArgs {
    pub session_id: String,
    pub task_id: String,
}

impl Tool for TaskAcceptTool {
    const NAME: &'static str = "macp_task_accept";
    type Error = MacpToolError;
    type Args = TaskAcceptArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_task_accept",
            "description": "Accept a task assignment in a MACP Task session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "task_id": { "type": "string" }
                },
                "required": ["session_id", "task_id"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::task_accept_payload(&args.task_id, &self.agent_id);
        let env = helpers::envelope(
            helpers::MODE_TASK,
            "TaskAccept",
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

// ── TaskCompleteTool ────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TaskCompleteTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct TaskCompleteArgs {
    pub session_id: String,
    pub task_id: String,
    pub summary: String,
}

impl Tool for TaskCompleteTool {
    const NAME: &'static str = "macp_task_complete";
    type Error = MacpToolError;
    type Args = TaskCompleteArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_task_complete",
            "description": "Mark a task as complete in a MACP Task session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "task_id": { "type": "string" },
                    "summary": { "type": "string" }
                },
                "required": ["session_id", "task_id", "summary"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::task_complete_payload(&args.task_id, &self.agent_id, &args.summary);
        let env = helpers::envelope(
            helpers::MODE_TASK,
            "TaskComplete",
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
