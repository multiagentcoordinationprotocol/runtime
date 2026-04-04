use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

use crate::helpers;
use super::SharedClient;

#[derive(Clone)]
pub struct StartSessionTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct StartSessionArgs {
    pub mode: String,
    pub session_id: String,
    pub intent: String,
    pub participants: Vec<String>,
    pub ttl_ms: i64,
}

#[derive(Serialize)]
pub struct StartSessionResult {
    pub ok: bool,
    pub session_id: String,
    pub session_state: i32,
}

#[derive(Debug, thiserror::Error)]
#[error("StartSession error: {0}")]
pub struct StartSessionError(String);

impl Tool for StartSessionTool {
    const NAME: &'static str = "macp_start_session";
    type Error = StartSessionError;
    type Args = StartSessionArgs;
    type Output = StartSessionResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_start_session",
            "description": "Start a new MACP coordination session with the given mode and participants",
            "parameters": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "description": "The MACP mode (e.g. macp.mode.decision.v1)" },
                    "session_id": { "type": "string", "description": "Unique session ID" },
                    "intent": { "type": "string", "description": "Purpose of the session" },
                    "participants": { "type": "array", "items": { "type": "string" }, "description": "List of participant agent IDs" },
                    "ttl_ms": { "type": "integer", "description": "Session time-to-live in milliseconds" }
                },
                "required": ["mode", "session_id", "intent", "participants", "ttl_ms"]
            }
        })).unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let participants: Vec<&str> = args.participants.iter().map(|s| s.as_str()).collect();
        let payload = helpers::session_start_payload(&args.intent, &participants, args.ttl_ms);
        let env = helpers::envelope(
            &args.mode,
            "SessionStart",
            &helpers::new_message_id(),
            &args.session_id,
            &self.agent_id,
            payload,
        );

        let mut client = self.client.lock().await;
        let ack = helpers::send_as(&mut client, &self.agent_id, env)
            .await
            .map_err(|e| StartSessionError(e.to_string()))?;

        Ok(StartSessionResult {
            ok: ack.ok,
            session_id: args.session_id,
            session_state: ack.session_state,
        })
    }
}
