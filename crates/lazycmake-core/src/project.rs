use std::path::{Path, PathBuf};

use crate::config::{AppConfig, ConfigOptions};
use crate::error::{Error, Result};
use crate::presets::PresetStore;

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub presets: Option<PresetStore>,
    pub config: AppConfig,
}

impl Project {
    pub fn discover(project_path: Option<&Path>, config_options: &ConfigOptions) -> Result<Self> {
        let root = resolve_project_root(project_path)?;
        let config = AppConfig::load(&root, config_options)?;
        let presets = if root.join("CMakePresets.json").exists() {
            Some(PresetStore::load(&root)?)
        } else {
            None
        };
        Ok(Self { root, presets, config })
    }

    pub fn has_presets(&self) -> bool {
        self.presets.is_some()
    }
}

pub fn resolve_project_root(path: Option<&Path>) -> Result<PathBuf> {
    let start = match path {
        Some(p) => p.canonicalize().map_err(|_| Error::ProjectNotFound(p.display().to_string()))?,
        None => std::env::current_dir()?,
    };

    let mut current = start.as_path();
    loop {
        if current.join("CMakePresets.json").is_file() || current.join("CMakeLists.txt").is_file() {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    Err(Error::ProjectNotFound(start.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_project_with_cmake_presets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CMakePresets.json"), r#"{"version":6,"configurePresets":[]}"#).unwrap();
        let root = resolve_project_root(Some(dir.path())).unwrap();
        assert_eq!(root, dir.path().canonicalize().unwrap());
    }
}
