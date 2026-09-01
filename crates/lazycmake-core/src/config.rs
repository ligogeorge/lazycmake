use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::env_file::load_env_file;
use crate::error::Result;
use crate::presets::expand_string;

#[derive(Debug, Clone, Default)]
pub struct ConfigOptions {
    /// Path to `config.toml`, or a directory containing it (e.g. `.zed/.lazycmake`).
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub testing: TestingConfig,
    #[serde(default)]
    pub presets: PresetsConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeneralConfig {
    pub default_preset: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TestingConfig {
    #[serde(default, rename = "curated_presets")]
    pub curated_presets: Option<Vec<String>>,
    #[serde(default)]
    pub presets: HashMap<String, TestingPresetConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TestingPresetConfig {
    pub label: Option<String>,
    pub test_dir: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresetsConfig {
    #[serde(default)]
    pub overrides: HashMap<String, PresetOverrideConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresetOverrideConfig {
    pub source_dir: Option<String>,
    pub generator: Option<String>,
    #[serde(default)]
    pub cache_variables: HashMap<String, String>,
    /// Extra environment variables for configure/build/run of this preset.
    /// Values may use `$env{NAME}` / `${sourceDir}` macros.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Shell script to `source` before jobs for this preset (e.g. a toolchain env file).
    /// Path may use `$env{NAME}` macros. Applied before `env` and before macro expansion.
    pub env_file: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UiConfig {
    pub theme: Option<String>,
}

impl AppConfig {
    pub fn load(project_root: &Path, options: &ConfigOptions) -> Result<Self> {
        let mut config = Self::default();

        if let Some(global) = global_config_path() {
            if global.exists() {
                merge_config(&mut config, &std::fs::read_to_string(global)?)?;
            }
        }

        if let Some(local) = resolve_project_config(project_root, options) {
            if local.exists() {
                merge_config(&mut config, &std::fs::read_to_string(local)?)?;
            }
        }

        Ok(config)
    }

    pub fn testing_preset(&self, name: &str) -> TestingPresetConfig {
        self.testing.presets.get(name).cloned().unwrap_or_default()
    }

    pub fn curated_test_presets(&self) -> Option<&Vec<String>> {
        self.testing.curated_presets.as_ref()
    }

    /// Configure preset that backs the Tests column / `t`/`T` actions.
    ///
    /// When `[testing].curated_presets` is set, that list owns the Tests column
    /// and the selected firmware configure preset is ignored. Prefer a curated
    /// name if the current selection is one of them; otherwise the first entry.
    /// With no curated list, follow the selected configure preset (spec §2).
    pub fn active_testing_preset(&self, selected_configure: Option<&str>) -> Option<String> {
        if let Some(curated) = self.curated_test_presets() {
            if curated.is_empty() {
                return None;
            }
            if let Some(sel) = selected_configure {
                if curated.iter().any(|c| c == sel) {
                    return Some(sel.to_string());
                }
            }
            return curated.first().cloned();
        }
        selected_configure.map(str::to_string)
    }

    pub fn resolve_test_dir(&self, preset_name: &str, binary_dir: &Path, project_root: &Path) -> PathBuf {
        if let Some(test_dir) = self.testing_preset(preset_name).test_dir {
            return project_root.join(test_dir);
        }
        if binary_dir.is_absolute() {
            binary_dir.to_path_buf()
        } else {
            project_root.join(binary_dir)
        }
    }

    /// Resolve the directory used for CTest discovery/run.
    ///
    /// `testing_binary_dir` is the active testing configure preset's binary
    /// dir when known (from `CMakePresets.json`). `selected_binary_dir` is only
    /// used when curation is off and the selected configure preset backs tests.
    pub fn resolve_testing_dir(
        &self,
        selected_configure: Option<&str>,
        testing_binary_dir: Option<&Path>,
        selected_binary_dir: &Path,
        project_root: &Path,
    ) -> Option<PathBuf> {
        let name = self.active_testing_preset(selected_configure)?;
        if let Some(test_dir) = self.testing_preset(&name).test_dir {
            return Some(project_root.join(test_dir));
        }
        if let Some(dir) = testing_binary_dir {
            return Some(self.resolve_test_dir(&name, dir, project_root));
        }
        if selected_configure == Some(name.as_str()) {
            return Some(self.resolve_test_dir(&name, selected_binary_dir, project_root));
        }
        None
    }

    pub fn preset_override(&self, name: &str) -> Option<&PresetOverrideConfig> {
        self.presets.overrides.get(name)
    }

    pub fn has_preset_override(&self, name: &str) -> bool {
        self.presets.overrides.contains_key(name)
    }

    /// Resolve `env_file` + `env` for a preset override into a concrete map.
    ///
    /// `env_file` is sourced first (if set); then `env` entries are expanded and merged on top.
    pub fn resolve_override_env(
        &self,
        preset_name: &str,
        project_root: &Path,
    ) -> Result<HashMap<String, String>> {
        let Some(ov) = self.preset_override(preset_name) else {
            return Ok(HashMap::new());
        };
        ov.resolve_env(project_root, preset_name)
    }
}

impl PresetOverrideConfig {
    pub fn resolve_env(
        &self,
        project_root: &Path,
        preset_name: &str,
    ) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        if let Some(raw) = &self.env_file {
            let path = PathBuf::from(expand_string(raw, project_root, preset_name)?);
            map.extend(load_env_file(&path)?);
        }
        for (key, raw) in &self.env {
            map.insert(key.clone(), expand_string(raw, project_root, preset_name)?);
        }
        Ok(map)
    }
}

/// Resolve project-local config path.
///
/// Priority:
/// 1. `--config` / `ConfigOptions::config_path` (file or directory)
/// 2. `<project>/.zed/.lazycmake/config.toml` (personal Zed config)
/// 3. `<project>/.lazycmake/config.toml` (project-local fallback)
pub fn resolve_project_config(project_root: &Path, options: &ConfigOptions) -> Option<PathBuf> {
    if let Some(path) = &options.config_path {
        return Some(normalize_config_path(path));
    }

    let zed_config = project_root.join(".zed/.lazycmake/config.toml");
    if zed_config.exists() {
        return Some(zed_config);
    }

    let project_config = project_root.join(".lazycmake/config.toml");
    if project_config.exists() {
        return Some(project_config);
    }

    None
}

fn normalize_config_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("config.toml")
    } else {
        path.to_path_buf()
    }
}

fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lazycmake/config.toml"))
}

fn merge_config(base: &mut AppConfig, contents: &str) -> Result<()> {
    let overlay: AppConfig = toml::from_str(contents)?;
    if overlay.general.default_preset.is_some() {
        base.general.default_preset = overlay.general.default_preset;
    }
    if overlay.testing.curated_presets.is_some() {
        base.testing.curated_presets = overlay.testing.curated_presets;
    }
    for (k, v) in overlay.testing.presets {
        base.testing.presets.insert(k, v);
    }
    for (k, v) in overlay.presets.overrides {
        base.presets.overrides.insert(k, v);
    }
    if overlay.ui.theme.is_some() {
        base.ui.theme = overlay.ui.theme;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn merge_local_overrides_global_defaults() {
        let toml = r#"
[general]
default_preset = "tests"

[testing]
curated_presets = ["tests"]

[testing.presets.tests]
test_dir = "build-test/src/tests"
extra_args = ["--output-on-failure", "--parallel"]
"#;
        let mut config = AppConfig::default();
        merge_config(&mut config, toml).unwrap();
        assert_eq!(config.general.default_preset.as_deref(), Some("tests"));
        let tp = config.testing_preset("tests");
        assert_eq!(tp.test_dir.as_deref(), Some("build-test/src/tests"));
        assert_eq!(tp.extra_args, vec!["--output-on-failure", "--parallel"]);
    }

    #[test]
    fn resolves_explicit_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_dir = dir.path().join(".lazycmake");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(cfg_dir.join("config.toml"), "[general]\ndefault_preset = \"x\"\n").unwrap();

        let options = ConfigOptions {
            config_path: Some(cfg_dir.clone()),
        };
        let resolved = resolve_project_config(dir.path(), &options).unwrap();
        assert_eq!(resolved, cfg_dir.join("config.toml"));
        let cfg = AppConfig::load(dir.path(), &options).unwrap();
        assert_eq!(cfg.general.default_preset.as_deref(), Some("x"));
    }

    #[test]
    fn prefers_zed_config_over_project_config() {
        let dir = tempfile::tempdir().unwrap();
        let zed_cfg = dir.path().join(".zed/.lazycmake");
        let proj_cfg = dir.path().join(".lazycmake");
        fs::create_dir_all(&zed_cfg).unwrap();
        fs::create_dir_all(&proj_cfg).unwrap();
        fs::write(zed_cfg.join("config.toml"), "[general]\ndefault_preset = \"zed\"\n").unwrap();
        fs::write(proj_cfg.join("config.toml"), "[general]\ndefault_preset = \"proj\"\n").unwrap();

        let resolved = resolve_project_config(dir.path(), &ConfigOptions::default()).unwrap();
        assert_eq!(resolved, zed_cfg.join("config.toml"));
        let cfg = AppConfig::load(dir.path(), &ConfigOptions::default()).unwrap();
        assert_eq!(cfg.general.default_preset.as_deref(), Some("zed"));
    }

    #[test]
    fn resolve_test_dir_uses_preset_override() {
        let mut config = AppConfig::default();
        config.testing.presets.insert(
            "tests".into(),
            TestingPresetConfig {
                test_dir: Some("build-test/src/tests".into()),
                ..Default::default()
            },
        );
        let root = Path::new("/proj");
        let dir = config.resolve_test_dir("tests", Path::new("build-test"), root);
        assert_eq!(dir, root.join("build-test/src/tests"));
    }

    #[test]
    fn resolve_test_dir_falls_back_to_binary_dir() {
        let config = AppConfig::default();
        let root = Path::new("/proj");
        let relative = config.resolve_test_dir("other", Path::new("build-x"), root);
        assert_eq!(relative, root.join("build-x"));

        let absolute = PathBuf::from("/abs/build");
        let resolved = config.resolve_test_dir("other", &absolute, root);
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn active_testing_preset_uses_curated_even_when_firmware_selected() {
        let mut config = AppConfig::default();
        config.testing.curated_presets = Some(vec!["tests".into()]);
        assert_eq!(
            config.active_testing_preset(Some("TRV8-2Full")).as_deref(),
            Some("tests")
        );
        assert_eq!(
            config.active_testing_preset(Some("tests")).as_deref(),
            Some("tests")
        );
        assert_eq!(config.active_testing_preset(None).as_deref(), Some("tests"));
    }

    #[test]
    fn active_testing_preset_without_curated_follows_selection() {
        let config = AppConfig::default();
        assert_eq!(
            config.active_testing_preset(Some("TRV8-2Full")).as_deref(),
            Some("TRV8-2Full")
        );
        assert_eq!(config.active_testing_preset(None), None);
    }

    #[test]
    fn resolve_testing_dir_uses_curated_preset_override() {
        let mut config = AppConfig::default();
        config.testing.curated_presets = Some(vec!["tests".into()]);
        config.testing.presets.insert(
            "tests".into(),
            TestingPresetConfig {
                test_dir: Some("build-test/src/tests".into()),
                ..Default::default()
            },
        );
        let root = Path::new("/proj");
        // Firmware binary dir must be ignored when curated testing is configured.
        let dir = config.resolve_testing_dir(
            Some("TRV8-2Full"),
            None,
            Path::new("build-nrfbm"),
            root,
        );
        assert_eq!(dir, Some(root.join("build-test/src/tests")));
    }

    #[test]
    fn merges_preset_overrides() {
        let toml = r#"
[presets.overrides.TRV8-2Full]
env_file = "$env{NRFBM_ENV_FILE}"
env = { EXTRA_FLAG = "1" }
source_dir = "$env{ZEPHYR_BASE}/share/sysbuild"
cache_variables = { APP_DIR = "${sourceDir}", BOARD = "trv8@1/nrf54l15/cpuapp/s115_softdevice/mcuboot" }
"#;
        let mut config = AppConfig::default();
        merge_config(&mut config, toml).unwrap();
        let ov = config.preset_override("TRV8-2Full").unwrap();
        assert_eq!(
            ov.source_dir.as_deref(),
            Some("$env{ZEPHYR_BASE}/share/sysbuild")
        );
        assert_eq!(ov.env_file.as_deref(), Some("$env{NRFBM_ENV_FILE}"));
        assert_eq!(ov.env.get("EXTRA_FLAG").map(String::as_str), Some("1"));
        assert_eq!(ov.cache_variables.get("APP_DIR").map(String::as_str), Some("${sourceDir}"));
        assert_eq!(
            ov.cache_variables.get("BOARD").map(String::as_str),
            Some("trv8@1/nrf54l15/cpuapp/s115_softdevice/mcuboot")
        );
        assert!(config.has_preset_override("TRV8-2Full"));
        assert!(!config.has_preset_override("tests"));
    }

    #[test]
    fn resolve_override_env_merges_file_and_map() {
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join("toolchain.sh");
        fs::write(&env_path, "export FROM_FILE=yes\nexport OVERRIDE_ME=file\n").unwrap();

        let mut config = AppConfig::default();
        config.presets.overrides.insert(
            "BoardX".into(),
            PresetOverrideConfig {
                env_file: Some(env_path.display().to_string()),
                env: HashMap::from([
                    ("OVERRIDE_ME".into(), "map".into()),
                    ("FROM_MAP".into(), "1".into()),
                ]),
                ..Default::default()
            },
        );

        let resolved = config.resolve_override_env("BoardX", dir.path()).unwrap();
        assert_eq!(resolved.get("FROM_FILE").map(String::as_str), Some("yes"));
        assert_eq!(resolved.get("FROM_MAP").map(String::as_str), Some("1"));
        assert_eq!(resolved.get("OVERRIDE_ME").map(String::as_str), Some("map"));
    }

    #[test]
    fn resolve_testing_dir_without_curated_uses_selected_binary_dir() {
        let config = AppConfig::default();
        let root = Path::new("/proj");
        let dir = config.resolve_testing_dir(
            Some("TRV8-2Full"),
            None,
            Path::new("build-nrfbm"),
            root,
        );
        assert_eq!(dir, Some(root.join("build-nrfbm")));
    }

    #[test]
    fn resolve_testing_dir_curated_without_test_dir_needs_testing_binary() {
        let mut config = AppConfig::default();
        config.testing.curated_presets = Some(vec!["tests".into()]);
        let root = Path::new("/proj");
        assert_eq!(
            config.resolve_testing_dir(Some("TRV8-2Full"), None, Path::new("build-nrfbm"), root),
            None
        );
        assert_eq!(
            config.resolve_testing_dir(
                Some("TRV8-2Full"),
                Some(Path::new("build-test")),
                Path::new("build-nrfbm"),
                root,
            ),
            Some(root.join("build-test"))
        );
    }
}
