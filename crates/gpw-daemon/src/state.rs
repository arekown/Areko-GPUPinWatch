//! Persistent state per GPU (phase + power limit before throttling) in `state.json`,
//! so a restarted service can give a throttled card its previous limit back.
use gpw_core::policy::Phase;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuState {
    pub phase: Phase,
    /// Limit before the first throttle; restored on release.
    pub prev_limit_w: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Key: normalised PCI address.
    pub gpus: BTreeMap<String, GpuState>,
}

pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join("state.json")
}

impl State {
    /// Missing or unreadable file = nothing to restore.
    pub fn load(file: &Path) -> State {
        std::fs::read_to_string(file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Atomic write (temp file + rename).
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(&tmp, file)
    }
}
