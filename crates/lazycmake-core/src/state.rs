use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FocusedColumn {
    #[default]
    Presets,
    Targets,
    Tests,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnState {
    pub filter: String,
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedState {
    pub last_preset: Option<String>,
    pub last_target: Option<String>,
    pub focused_column: FocusedColumn,
    pub presets: ColumnState,
    pub targets: ColumnState,
    pub tests: ColumnState,
    pub tests_failing_only: bool,
}

impl PersistedState {
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    /// Load from `path`, falling back to `legacy_paths` when the primary file is missing.
    pub fn load_with_fallbacks(path: &Path, legacy_paths: &[&Path]) -> Result<Self> {
        if path.exists() {
            return Self::load_from(path);
        }
        for legacy in legacy_paths {
            if legacy.exists() {
                return Self::load_from(legacy);
            }
        }
        Ok(Self::default())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_state_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/state.json");
        let state = PersistedState::load_from(&path).unwrap();
        assert!(state.last_preset.is_none());
        assert_eq!(state.focused_column, FocusedColumn::Presets);
        assert!(!state.tests_failing_only);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom/state.json");
        let mut state = PersistedState::default();
        state.last_preset = Some("tests".into());
        state.last_target = Some("tracker_app".into());
        state.focused_column = FocusedColumn::Tests;
        state.tests.selected = 7;
        state.tests_failing_only = true;
        state.save_to(&path).unwrap();

        let loaded = PersistedState::load_from(&path).unwrap();
        assert_eq!(loaded.last_preset.as_deref(), Some("tests"));
        assert_eq!(loaded.last_target.as_deref(), Some("tracker_app"));
        assert_eq!(loaded.focused_column, FocusedColumn::Tests);
        assert_eq!(loaded.tests.selected, 7);
        assert!(loaded.tests_failing_only);
        assert!(path.exists());
    }

    #[test]
    fn load_falls_back_to_legacy_path() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary/state.json");
        let legacy = dir.path().join("legacy/state.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let mut state = PersistedState::default();
        state.last_preset = Some("legacy".into());
        std::fs::write(&legacy, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let loaded = PersistedState::load_with_fallbacks(&primary, &[&legacy]).unwrap();
        assert_eq!(loaded.last_preset.as_deref(), Some("legacy"));

        loaded.save_to(&primary).unwrap();
        assert!(primary.exists());
    }
}
