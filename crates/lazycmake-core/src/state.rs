use std::path::{Path, PathBuf};

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
    pub fn path(project_root: &Path) -> PathBuf {
        project_root.join(".lazycmake/state.json")
    }

    pub fn load(project_root: &Path) -> Result<Self> {
        let path = Self::path(project_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    pub fn save(&self, project_root: &Path) -> Result<()> {
        let dir = project_root.join(".lazycmake");
        std::fs::create_dir_all(&dir)?;
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(dir.join("state.json"), contents)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_state_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let state = PersistedState::load(dir.path()).unwrap();
        assert!(state.last_preset.is_none());
        assert_eq!(state.focused_column, FocusedColumn::Presets);
        assert!(!state.tests_failing_only);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PersistedState::default();
        state.last_preset = Some("tests".into());
        state.last_target = Some("tracker_app".into());
        state.focused_column = FocusedColumn::Tests;
        state.tests.selected = 7;
        state.tests_failing_only = true;
        state.save(dir.path()).unwrap();

        let loaded = PersistedState::load(dir.path()).unwrap();
        assert_eq!(loaded.last_preset.as_deref(), Some("tests"));
        assert_eq!(loaded.last_target.as_deref(), Some("tracker_app"));
        assert_eq!(loaded.focused_column, FocusedColumn::Tests);
        assert_eq!(loaded.tests.selected, 7);
        assert!(loaded.tests_failing_only);
        assert_eq!(PersistedState::path(dir.path()), dir.path().join(".lazycmake/state.json"));
    }
}
