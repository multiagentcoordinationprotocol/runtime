use crate::pb::Envelope;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::broadcast;

const DEFAULT_SESSION_STREAM_CAPACITY: usize = 256;

pub struct SessionStreamBus {
    channels: Mutex<HashMap<String, broadcast::Sender<Envelope>>>,
    capacity: usize,
}

impl Default for SessionStreamBus {
    fn default() -> Self {
        Self::new(DEFAULT_SESSION_STREAM_CAPACITY)
    }
}

impl SessionStreamBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    pub fn subscribe(&self, session_id: &str) -> broadcast::Receiver<Envelope> {
        let mut guard = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| {
                let (sender, _receiver) = broadcast::channel(self.capacity);
                sender
            })
            .subscribe()
    }

    pub fn publish(&self, session_id: &str, envelope: Envelope) {
        let sender = {
            let guard = self.channels.lock().unwrap_or_else(|e| e.into_inner());
            guard.get(session_id).cloned()
        };
        if let Some(sender) = sender {
            let _ = sender.send(envelope);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(message_id: &str) -> Envelope {
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.decision.v1".into(),
            message_type: "Proposal".into(),
            message_id: message_id.into(),
            session_id: "s1".into(),
            sender: "agent://sender".into(),
            timestamp_unix_ms: 1,
            payload: vec![],
        }
    }

    #[test]
    fn subscribe_then_publish_round_trip() {
        let bus = SessionStreamBus::default();
        let mut rx = bus.subscribe("s1");
        bus.publish("s1", env("m1"));
        let envelope = rx.try_recv().expect("stream event");
        assert_eq!(envelope.message_id, "m1");
    }
}
