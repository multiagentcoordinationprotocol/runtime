use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::broadcast;

use crate::mode::decision::DecisionMode;
use crate::mode::handoff::HandoffMode;
use crate::mode::multi_round::MultiRoundMode;
use crate::mode::passthrough::PassthroughMode;
use crate::mode::proposal::ProposalMode;
use crate::mode::quorum::QuorumMode;
use crate::mode::task::TaskMode;
use crate::mode::{
    extension_mode_descriptors, standard_mode_descriptors, Mode, STANDARD_MODE_NAMES,
};
use crate::pb::ModeDescriptor;

pub struct ModeRegistration {
    pub mode_name: String,
    pub mode: Box<dyn Mode>,
    pub descriptor: Option<ModeDescriptor>,
    pub standards_track: bool,
    pub builtin: bool,
}

pub struct ModeRegistry {
    entries: RwLock<HashMap<String, ModeRegistration>>,
    change_tx: broadcast::Sender<()>,
}

impl ModeRegistry {
    /// Build the default registry with 5 standards-track modes and 1 built-in extension.
    pub fn build_default() -> Self {
        let std_descriptors = standard_mode_descriptors();
        let ext_descriptors = extension_mode_descriptors();
        let mut descriptor_map: HashMap<String, ModeDescriptor> = std_descriptors
            .into_iter()
            .chain(ext_descriptors)
            .map(|d| (d.mode.clone(), d))
            .collect();

        let mut entries = HashMap::new();

        // Standards-track modes
        let standard_modes: Vec<(&str, Box<dyn Mode>)> = vec![
            ("macp.mode.decision.v1", Box::new(DecisionMode)),
            ("macp.mode.proposal.v1", Box::new(ProposalMode)),
            ("macp.mode.task.v1", Box::new(TaskMode)),
            ("macp.mode.handoff.v1", Box::new(HandoffMode)),
            ("macp.mode.quorum.v1", Box::new(QuorumMode)),
        ];

        for (name, mode) in standard_modes {
            entries.insert(
                name.to_string(),
                ModeRegistration {
                    mode_name: name.to_string(),
                    mode,
                    descriptor: descriptor_map.remove(name),
                    standards_track: true,
                    builtin: true,
                },
            );
        }

        // Built-in extension modes
        let extension_modes: Vec<(&str, Box<dyn Mode>)> =
            vec![("ext.multi_round.v1", Box::new(MultiRoundMode))];

        for (name, mode) in extension_modes {
            entries.insert(
                name.to_string(),
                ModeRegistration {
                    mode_name: name.to_string(),
                    mode,
                    descriptor: descriptor_map.remove(name),
                    standards_track: false,
                    builtin: true,
                },
            );
        }

        let (change_tx, _) = broadcast::channel(16);
        Self {
            entries: RwLock::new(entries),
            change_tx,
        }
    }

    pub fn get_mode(&self, name: &str) -> Option<ModeRef<'_>> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        if guard.contains_key(name) {
            Some(ModeRef {
                registry: self,
                name: name.to_string(),
            })
        } else {
            None
        }
    }

    /// Execute a mode callback while holding the read lock.
    pub fn with_mode<F, R>(&self, name: &str, f: F) -> Option<R>
    where
        F: FnOnce(&dyn Mode) -> R,
    {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        guard.get(name).map(|e| f(e.mode.as_ref()))
    }

    pub fn standard_mode_names(&self) -> Vec<String> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        STANDARD_MODE_NAMES
            .iter()
            .filter(|name| guard.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect()
    }

    pub fn standard_mode_descriptors(&self) -> Vec<ModeDescriptor> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        STANDARD_MODE_NAMES
            .iter()
            .filter_map(|name| guard.get(*name).and_then(|e| e.descriptor.clone()))
            .collect()
    }

    pub fn extension_mode_names(&self) -> Vec<String> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        guard
            .iter()
            .filter(|(_, e)| !e.standards_track)
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn extension_mode_descriptors(&self) -> Vec<ModeDescriptor> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        guard
            .iter()
            .filter(|(_, e)| !e.standards_track)
            .filter_map(|(_, e)| e.descriptor.clone())
            .collect()
    }

    pub fn all_mode_names(&self) -> Vec<String> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        guard.keys().cloned().collect()
    }

    pub fn all_mode_descriptors(&self) -> Vec<ModeDescriptor> {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        guard
            .values()
            .filter_map(|e| e.descriptor.clone())
            .collect()
    }

    pub fn is_standard_mode(&self, name: &str) -> bool {
        let guard = self.entries.read().expect("mode registry lock poisoned");
        guard.get(name).map(|e| e.standards_track).unwrap_or(false)
    }

    /// Register a new extension mode dynamically.
    pub fn register_extension(&self, descriptor: ModeDescriptor) -> Result<(), String> {
        let name = descriptor.mode.clone();
        if name.is_empty() {
            return Err("mode name must not be empty".into());
        }
        if name.starts_with("macp.mode.") {
            return Err("cannot register extension with reserved macp.mode.* namespace".into());
        }

        let allowed_types = descriptor
            .message_types
            .iter()
            .filter(|t| *t != "SessionStart")
            .cloned()
            .collect();
        let mode: Box<dyn Mode> = Box::new(PassthroughMode {
            allowed_message_types: allowed_types,
        });

        let mut guard = self.entries.write().expect("mode registry lock poisoned");
        if guard.contains_key(&name) {
            return Err(format!("mode '{}' is already registered", name));
        }
        guard.insert(
            name.clone(),
            ModeRegistration {
                mode_name: name,
                mode,
                descriptor: Some(descriptor),
                standards_track: false,
                builtin: false,
            },
        );
        drop(guard);
        let _ = self.change_tx.send(());
        Ok(())
    }

    /// Unregister a dynamically registered extension mode.
    pub fn unregister_extension(&self, mode: &str) -> Result<(), String> {
        let mut guard = self.entries.write().expect("mode registry lock poisoned");
        match guard.get(mode) {
            None => return Err(format!("mode '{}' not found", mode)),
            Some(entry) if entry.builtin => {
                return Err(format!("cannot unregister built-in mode '{}'", mode))
            }
            Some(entry) if entry.standards_track => {
                return Err(format!("cannot unregister standards-track mode '{}'", mode))
            }
            _ => {}
        }
        guard.remove(mode);
        drop(guard);
        let _ = self.change_tx.send(());
        Ok(())
    }

    /// Promote an extension mode to standards-track.
    /// Optionally re-keys the entry with a new identifier.
    pub fn promote_mode(&self, mode: &str, new_name: Option<&str>) -> Result<String, String> {
        let mut guard = self.entries.write().expect("mode registry lock poisoned");
        let entry = guard
            .get(mode)
            .ok_or_else(|| format!("mode '{}' not found", mode))?;
        if entry.standards_track {
            return Err(format!("mode '{}' is already standards-track", mode));
        }

        let final_name = new_name.unwrap_or(mode).to_string();
        if final_name != mode && guard.contains_key(&final_name) {
            return Err(format!(
                "cannot promote: target name '{}' already exists",
                final_name
            ));
        }

        // Remove old entry and re-insert with updated flags
        let mut registration = guard
            .remove(mode)
            .ok_or_else(|| format!("mode '{}' not found", mode))?;
        registration.standards_track = true;
        registration.mode_name = final_name.clone();
        if let Some(ref mut desc) = registration.descriptor {
            desc.mode = final_name.clone();
        }
        guard.insert(final_name.clone(), registration);
        drop(guard);
        let _ = self.change_tx.send(());
        Ok(final_name)
    }

    /// Subscribe to mode registry change notifications.
    pub fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }
}

/// A handle that allows calling mode methods while the registry read lock is not held
/// across await points. Callers use `with_mode` or obtain a `ModeRef` and call its methods.
pub struct ModeRef<'a> {
    registry: &'a ModeRegistry,
    name: String,
}

impl<'a> ModeRef<'a> {
    pub fn on_session_start(
        &self,
        session: &crate::session::Session,
        env: &crate::pb::Envelope,
    ) -> Result<crate::mode::ModeResponse, crate::error::MacpError> {
        let guard = self
            .registry
            .entries
            .read()
            .expect("mode registry lock poisoned");
        let entry = guard
            .get(&self.name)
            .ok_or(crate::error::MacpError::UnknownMode)?;
        entry.mode.on_session_start(session, env)
    }

    pub fn on_message(
        &self,
        session: &crate::session::Session,
        env: &crate::pb::Envelope,
    ) -> Result<crate::mode::ModeResponse, crate::error::MacpError> {
        let guard = self
            .registry
            .entries
            .read()
            .expect("mode registry lock poisoned");
        let entry = guard
            .get(&self.name)
            .ok_or(crate::error::MacpError::UnknownMode)?;
        entry.mode.on_message(session, env)
    }

    pub fn authorize_sender(
        &self,
        session: &crate::session::Session,
        env: &crate::pb::Envelope,
    ) -> Result<(), crate::error::MacpError> {
        let guard = self
            .registry
            .entries
            .read()
            .expect("mode registry lock poisoned");
        let entry = guard
            .get(&self.name)
            .ok_or(crate::error::MacpError::UnknownMode)?;
        entry.mode.authorize_sender(session, env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::EXTENSION_MODE_NAMES;

    #[test]
    fn build_default_contains_all_standard_modes() {
        let registry = ModeRegistry::build_default();
        for name in STANDARD_MODE_NAMES {
            assert!(registry.get_mode(name).is_some(), "missing mode: {name}");
            assert!(registry.is_standard_mode(name));
        }
    }

    #[test]
    fn build_default_contains_multi_round_as_extension() {
        let registry = ModeRegistry::build_default();
        assert!(registry.get_mode("ext.multi_round.v1").is_some());
        assert!(!registry.is_standard_mode("ext.multi_round.v1"));
    }

    #[test]
    fn standard_mode_names_returns_five() {
        let registry = ModeRegistry::build_default();
        assert_eq!(registry.standard_mode_names().len(), 5);
    }

    #[test]
    fn standard_mode_descriptors_returns_five() {
        let registry = ModeRegistry::build_default();
        assert_eq!(registry.standard_mode_descriptors().len(), 5);
    }

    #[test]
    fn all_mode_names_returns_six() {
        let registry = ModeRegistry::build_default();
        assert_eq!(registry.all_mode_names().len(), 6);
    }

    #[test]
    fn extension_mode_names_returns_one() {
        let registry = ModeRegistry::build_default();
        let ext = registry.extension_mode_names();
        assert_eq!(ext.len(), 1);
        assert!(ext.contains(&"ext.multi_round.v1".to_string()));
    }

    #[test]
    fn extension_mode_descriptors_returns_one() {
        let registry = ModeRegistry::build_default();
        let descs = registry.extension_mode_descriptors();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].mode, "ext.multi_round.v1");
    }

    #[test]
    fn unknown_mode_returns_none() {
        let registry = ModeRegistry::build_default();
        assert!(registry.get_mode("nonexistent").is_none());
        assert!(!registry.is_standard_mode("nonexistent"));
    }

    #[test]
    fn register_extension_mode() {
        let registry = ModeRegistry::build_default();
        let descriptor = ModeDescriptor {
            mode: "ext.custom.v1".into(),
            mode_version: "1.0.0".into(),
            title: "Custom Mode".into(),
            description: "Test custom mode".into(),
            message_types: vec![
                "SessionStart".into(),
                "CustomMsg".into(),
                "Commitment".into(),
            ],
            terminal_message_types: vec!["Commitment".into()],
            ..Default::default()
        };
        registry.register_extension(descriptor).unwrap();
        assert!(registry.get_mode("ext.custom.v1").is_some());
        assert!(!registry.is_standard_mode("ext.custom.v1"));
        assert_eq!(registry.all_mode_names().len(), 7);
        assert_eq!(registry.extension_mode_names().len(), 2);
    }

    #[test]
    fn register_rejects_macp_namespace() {
        let registry = ModeRegistry::build_default();
        let descriptor = ModeDescriptor {
            mode: "macp.mode.evil.v1".into(),
            ..Default::default()
        };
        assert!(registry.register_extension(descriptor).is_err());
    }

    #[test]
    fn register_rejects_duplicate() {
        let registry = ModeRegistry::build_default();
        let descriptor = ModeDescriptor {
            mode: "ext.multi_round.v1".into(),
            ..Default::default()
        };
        assert!(registry.register_extension(descriptor).is_err());
    }

    #[test]
    fn unregister_extension_mode() {
        let registry = ModeRegistry::build_default();
        let descriptor = ModeDescriptor {
            mode: "ext.temp.v1".into(),
            mode_version: "1.0.0".into(),
            message_types: vec!["SessionStart".into(), "Commitment".into()],
            ..Default::default()
        };
        registry.register_extension(descriptor).unwrap();
        assert_eq!(registry.all_mode_names().len(), 7);
        registry.unregister_extension("ext.temp.v1").unwrap();
        assert_eq!(registry.all_mode_names().len(), 6);
        assert!(registry.get_mode("ext.temp.v1").is_none());
    }

    #[test]
    fn cannot_unregister_builtin() {
        let registry = ModeRegistry::build_default();
        assert!(registry.unregister_extension("ext.multi_round.v1").is_err());
        assert!(registry
            .unregister_extension("macp.mode.decision.v1")
            .is_err());
    }

    #[test]
    fn promote_extension_to_standard() {
        let registry = ModeRegistry::build_default();
        let descriptor = ModeDescriptor {
            mode: "ext.new.v1".into(),
            mode_version: "1.0.0".into(),
            message_types: vec!["SessionStart".into(), "Commitment".into()],
            ..Default::default()
        };
        registry.register_extension(descriptor).unwrap();
        assert!(!registry.is_standard_mode("ext.new.v1"));

        let final_name = registry
            .promote_mode("ext.new.v1", Some("macp.mode.new.v1"))
            .unwrap();
        assert_eq!(final_name, "macp.mode.new.v1");
        assert!(registry.is_standard_mode("macp.mode.new.v1"));
        assert!(registry.get_mode("ext.new.v1").is_none());
        assert!(registry.get_mode("macp.mode.new.v1").is_some());
    }

    #[test]
    fn promote_without_rename() {
        let registry = ModeRegistry::build_default();
        let descriptor = ModeDescriptor {
            mode: "ext.keep.v1".into(),
            mode_version: "1.0.0".into(),
            message_types: vec!["SessionStart".into(), "Commitment".into()],
            ..Default::default()
        };
        registry.register_extension(descriptor).unwrap();
        let final_name = registry.promote_mode("ext.keep.v1", None).unwrap();
        assert_eq!(final_name, "ext.keep.v1");
        assert!(registry.is_standard_mode("ext.keep.v1"));
    }

    #[test]
    fn promote_already_standard_fails() {
        let registry = ModeRegistry::build_default();
        assert!(registry
            .promote_mode("macp.mode.decision.v1", None)
            .is_err());
    }

    #[test]
    fn extension_names_constant_matches() {
        for name in EXTENSION_MODE_NAMES {
            let registry = ModeRegistry::build_default();
            assert!(
                registry.get_mode(name).is_some(),
                "EXTENSION_MODE_NAMES entry missing from registry: {name}"
            );
        }
    }
}
