use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

use super::decision::MacpToolError;
use super::SharedClient;
use crate::helpers;

#[derive(Clone)]
pub struct GetSessionTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct GetSessionArgs {
    pub session_id: String,
}

#[derive(Serialize)]
pub struct GetSessionResult {
    pub session_id: String,
    pub mode: String,
    pub state: i32,
}

impl Tool for GetSessionTool {
    const NAME: &'static str = "macp_get_session";
    type Error = MacpToolError;
    type Args = GetSessionArgs;
    type Output = GetSessionResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_get_session",
            "description": "Get the current state of a MACP session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"]
            }
        }))
        .unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut client = self.client.lock().await;
        let resp = helpers::get_session_as(&mut client, &self.agent_id, &args.session_id)
            .await
            .map_err(|e| MacpToolError(e.to_string()))?;
        let meta = resp
            .metadata
            .ok_or_else(|| MacpToolError("no metadata".into()))?;
        Ok(GetSessionResult {
            session_id: meta.session_id,
            mode: meta.mode,
            state: meta.state,
        })
    }
}
