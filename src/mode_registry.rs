use std::collections::HashMap;

use crate::mode::decision::DecisionMode;
use crate::mode::handoff::HandoffMode;
use crate::mode::multi_round::MultiRoundMode;
use crate::mode::proposal::ProposalMode;
use crate::mode::quorum::QuorumMode;
use crate::mode::task::TaskMode;
use crate::mode::{standard_mode_descriptors, Mode, STANDARD_MODE_NAMES};
use crate::pb::ModeDescriptor;

pub struct ModeRegistration {
    pub mode_name: String,
    pub mode: Box<dyn Mode>,
    pub descriptor: Option<ModeDescriptor>,
    pub standards_track: bool,
}

pub struct ModeRegistry {
    entries: HashMap<String, ModeRegistration>,
}

impl ModeRegistry {
    /// Build the default registry with all 5 standard + 1 experimental modes.
    pub fn build_default() -> Self {
        let descriptors = standard_mode_descriptors();
        let descriptor_map: HashMap<String, ModeDescriptor> = descriptors
            .into_iter()
            .map(|d| (d.mode.clone(), d))
            .collect();

        let mut entries = HashMap::new();

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
                    descriptor: descriptor_map.get(name).cloned(),
                    standards_track: true,
                },
            );
        }

        // Experimental mode
        entries.insert(
            "macp.mode.multi_round.v1".to_string(),
            ModeRegistration {
                mode_name: "macp.mode.multi_round.v1".to_string(),
                mode: Box::new(MultiRoundMode),
                descriptor: None,
                standards_track: false,
            },
        );

        Self { entries }
    }

    pub fn get_mode(&self, name: &str) -> Option<&dyn Mode> {
        self.entries.get(name).map(|e| e.mode.as_ref())
    }

    pub fn standard_mode_names(&self) -> Vec<String> {
        STANDARD_MODE_NAMES
            .iter()
            .filter(|name| self.entries.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect()
    }

    pub fn standard_mode_descriptors(&self) -> Vec<ModeDescriptor> {
        STANDARD_MODE_NAMES
            .iter()
            .filter_map(|name| self.entries.get(*name).and_then(|e| e.descriptor.clone()))
            .collect()
    }

    pub fn is_standard_mode(&self, name: &str) -> bool {
        self.entries
            .get(name)
            .map(|e| e.standards_track)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_default_contains_all_standard_modes() {
        let registry = ModeRegistry::build_default();
        for name in STANDARD_MODE_NAMES {
            assert!(registry.get_mode(name).is_some(), "missing mode: {name}");
            assert!(registry.is_standard_mode(name));
        }
    }

    #[test]
    fn build_default_contains_experimental_mode() {
        let registry = ModeRegistry::build_default();
        assert!(registry.get_mode("macp.mode.multi_round.v1").is_some());
        assert!(!registry.is_standard_mode("macp.mode.multi_round.v1"));
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
    fn unknown_mode_returns_none() {
        let registry = ModeRegistry::build_default();
        assert!(registry.get_mode("nonexistent").is_none());
        assert!(!registry.is_standard_mode("nonexistent"));
    }
}
