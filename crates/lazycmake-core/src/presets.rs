use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfigurePreset {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub generator: Option<String>,
    pub binary_dir: PathBuf,
    pub cache_variables: HashMap<String, String>,
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub struct PresetStore {
    source_dir: PathBuf,
    presets: HashMap<String, ResolvedConfigurePreset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetsFile {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    configure_presets: Vec<RawConfigurePreset>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RawConfigurePreset {
    name: String,
    #[serde(default)]
    inherits: Option<Value>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    generator: Option<String>,
    #[serde(default)]
    binary_dir: Option<String>,
    #[serde(default)]
    cache_variables: Option<HashMap<String, Value>>,
    #[serde(default)]
    condition: Option<PresetCondition>,
}

#[derive(Debug, Deserialize, Clone)]
struct PresetCondition {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    lhs: Option<String>,
    #[serde(default)]
    rhs: Option<String>,
}

impl PresetStore {
    pub fn load(source_dir: &Path) -> Result<Self> {
        let mut raw_map: HashMap<String, RawConfigurePreset> = HashMap::new();

        let main_path = source_dir.join("CMakePresets.json");
        if main_path.exists() {
            merge_file(source_dir, &main_path, &mut raw_map)?;
        }

        let user_path = source_dir.join("CMakeUserPresets.json");
        if user_path.exists() {
            merge_file(source_dir, &user_path, &mut raw_map)?;
        }

        let mut resolved = HashMap::new();
        for name in raw_map.keys().cloned().collect::<Vec<_>>() {
            if let Some(raw) = raw_map.get(&name) {
                if !evaluate_condition(raw) {
                    continue;
                }
                let preset = resolve_preset(name.clone(), raw, &raw_map, source_dir)?;
                resolved.insert(name, preset);
            }
        }

        Ok(Self {
            source_dir: source_dir.to_path_buf(),
            presets: resolved,
        })
    }

    pub fn visible_configure_presets(&self) -> Vec<&ResolvedConfigurePreset> {
        self.visible_configure_presets_with_overrides(&[])
    }

    /// Presets shown in the UI: non-hidden, plus any hidden presets that have
    /// a config override (so `[presets.overrides.<name>]` can unhide them).
    pub fn visible_configure_presets_with_overrides(
        &self,
        override_names: &[&str],
    ) -> Vec<&ResolvedConfigurePreset> {
        let mut presets: Vec<_> = self
            .presets
            .values()
            .filter(|p| !p.hidden || override_names.iter().any(|n| *n == p.name))
            .collect();
        presets.sort_by(|a, b| a.name.cmp(&b.name));
        presets
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedConfigurePreset> {
        self.presets.get(name)
    }

    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    pub fn resolve_binary_dir(&self, preset: &ResolvedConfigurePreset) -> PathBuf {
        if preset.binary_dir.is_absolute() {
            preset.binary_dir.clone()
        } else {
            self.source_dir.join(&preset.binary_dir)
        }
    }
}

fn merge_file(base: &Path, path: &Path, raw_map: &mut HashMap<String, RawConfigurePreset>) -> Result<()> {
    let contents = std::fs::read_to_string(path)?;
    let file: PresetsFile = serde_json::from_str(&contents)?;

    for include in &file.include {
        let include_path = base.join(include);
        if include_path.exists() {
            merge_file(base, &include_path, raw_map)?;
        }
    }

    for preset in file.configure_presets {
        raw_map.insert(preset.name.clone(), preset);
    }

    Ok(())
}

fn evaluate_condition(raw: &RawConfigurePreset) -> bool {
    let Some(cond) = &raw.condition else {
        return true;
    };
    match cond.kind.as_str() {
        "equals" => cond.lhs.as_deref() == cond.rhs.as_deref(),
        "notEquals" => cond.lhs.as_deref() != cond.rhs.as_deref(),
        "const" => cond.lhs.as_deref() == Some("true"),
        _ => true,
    }
}

fn resolve_preset(
    name: String,
    raw: &RawConfigurePreset,
    all: &HashMap<String, RawConfigurePreset>,
    source_dir: &Path,
) -> Result<ResolvedConfigurePreset> {
    let mut merged = MergedPreset::default();

    if let Some(inherits) = &raw.inherits {
        for parent in parse_inherits(inherits) {
            let parent_raw = all
                .get(&parent)
                .ok_or_else(|| Error::Parse(format!("preset '{name}' inherits unknown '{parent}'")))?;
            let parent_resolved = resolve_preset(parent.clone(), parent_raw, all, source_dir)?;
            merged.apply_resolved(&parent_resolved);
        }
    }

    merged.display_name = raw.display_name.clone().or(merged.display_name);
    merged.description = raw.description.clone().or(merged.description);
    if raw.generator.is_some() {
        merged.generator = raw.generator.clone();
    }
    if raw.binary_dir.is_some() {
        merged.binary_dir = raw.binary_dir.clone();
    }
    if let Some(vars) = &raw.cache_variables {
        for (k, v) in vars {
            merged.cache_variables.insert(k.clone(), cache_value_to_string(v)?);
        }
    }
    merged.hidden = raw.hidden;

    let ctx = ExpandContext {
        source_dir,
        preset_name: &name,
    };

    let binary_dir = expand_macros(merged.binary_dir.as_deref().unwrap_or("build"), &ctx)?;
    let mut cache_variables = HashMap::new();
    for (k, v) in merged.cache_variables {
        cache_variables.insert(k, expand_macros(&v, &ctx)?);
    }

    Ok(ResolvedConfigurePreset {
        name,
        display_name: merged.display_name,
        description: merged.description,
        generator: merged.generator,
        binary_dir: PathBuf::from(binary_dir),
        cache_variables,
        hidden: merged.hidden,
    })
}

#[derive(Default)]
struct MergedPreset {
    display_name: Option<String>,
    description: Option<String>,
    generator: Option<String>,
    binary_dir: Option<String>,
    cache_variables: HashMap<String, String>,
    hidden: bool,
}

impl MergedPreset {
    fn apply_resolved(&mut self, parent: &ResolvedConfigurePreset) {
        if self.display_name.is_none() {
            self.display_name = parent.display_name.clone();
        }
        if self.description.is_none() {
            self.description = parent.description.clone();
        }
        if self.generator.is_none() {
            self.generator = parent.generator.clone();
        }
        if self.binary_dir.is_none() {
            self.binary_dir = Some(parent.binary_dir.display().to_string());
        }
        for (k, v) in &parent.cache_variables {
            self.cache_variables.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

fn parse_inherits(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(arr) => arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

struct ExpandContext<'a> {
    source_dir: &'a Path,
    preset_name: &'a str,
}

fn expand_macros(input: &str, ctx: &ExpandContext<'_>) -> Result<String> {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("${sourceDir}") {
            out.push_str(&ctx.source_dir.display().to_string());
            rest = &rest["${sourceDir}".len()..];
        } else if rest.starts_with("${presetName}") {
            out.push_str(ctx.preset_name);
            rest = &rest["${presetName}".len()..];
        } else if rest.starts_with("$env{") {
            let end = rest.find('}').ok_or_else(|| Error::Parse("unclosed $env{}".into()))?;
            let var = &rest[5..end];
            let val = std::env::var(var).map_err(|_| Error::EnvVarMissing(var.to_string()))?;
            out.push_str(&val);
            rest = &rest[end + 1..];
        } else if rest.starts_with("$penv{") {
            let end = rest.find('}').ok_or_else(|| Error::Parse("unclosed $penv{}".into()))?;
            let var = &rest[6..end];
            let val = std::env::var(var).map_err(|_| Error::EnvVarMissing(var.to_string()))?;
            out.push_str(&val);
            rest = &rest[end + 1..];
        } else {
            out.push('$');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Expand `${sourceDir}` / `$env{VAR}` macros relative to the project source dir.
pub fn expand_string(input: &str, source_dir: &Path, preset_name: &str) -> Result<String> {
    expand_macros(
        input,
        &ExpandContext {
            source_dir,
            preset_name,
        },
    )
}

fn cache_value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::String(s) => Ok(s.clone()),
        Value::Object(obj) => {
            if let Some(v) = obj.get("value") {
                cache_value_to_string(v)
            } else {
                Ok(value.to_string())
            }
        }
        _ => Ok(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_inherits_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{
  "version": 6,
  "configurePresets": [
    {
      "name": "base",
      "hidden": true,
      "generator": "Ninja",
      "binaryDir": "build-base",
      "cacheVariables": { "FOO": "1" }
    },
    {
      "name": "child",
      "inherits": "base",
      "cacheVariables": { "BAR": "2" }
    },
    {
      "name": "hiddenChild",
      "inherits": "base",
      "hidden": true
    }
  ]
}"#;
        fs::write(dir.path().join("CMakePresets.json"), json).unwrap();
        let store = PresetStore::load(dir.path()).unwrap();
        let visible: Vec<_> = store
            .visible_configure_presets()
            .into_iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(visible, vec!["child"]);
        let child = store.get("child").unwrap();
        assert_eq!(child.generator.as_deref(), Some("Ninja"));
        assert_eq!(child.cache_variables.get("FOO").map(String::as_str), Some("1"));
        assert_eq!(child.cache_variables.get("BAR").map(String::as_str), Some("2"));
    }

    #[test]
    fn override_reveals_hidden_preset() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{
  "version": 6,
  "configurePresets": [
    { "name": "shown", "binaryDir": "b1" },
    { "name": "TRV8-2Full", "hidden": true, "binaryDir": "build-nrfbm", "generator": "Ninja" }
  ]
}"#;
        fs::write(dir.path().join("CMakePresets.json"), json).unwrap();
        let store = PresetStore::load(dir.path()).unwrap();
        let without: Vec<_> = store
            .visible_configure_presets()
            .into_iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(without, vec!["shown"]);
        let with: Vec<_> = store
            .visible_configure_presets_with_overrides(&["TRV8-2Full"])
            .into_iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(with, vec!["TRV8-2Full", "shown"]);
    }

    #[test]
    fn expands_source_dir_and_env() {
        std::env::set_var("LAZYCMAKE_TEST_VAR", "hello");
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{
  "version": 6,
  "configurePresets": [
    {
      "name": "p",
      "binaryDir": "${sourceDir}/build",
      "cacheVariables": { "X": "$env{LAZYCMAKE_TEST_VAR}" }
    }
  ]
}"#;
        fs::write(dir.path().join("CMakePresets.json"), json).unwrap();
        let store = PresetStore::load(dir.path()).unwrap();
        let preset = store.get("p").unwrap();
        assert!(preset.binary_dir.ends_with("build"));
        assert_eq!(preset.cache_variables.get("X").map(String::as_str), Some("hello"));
    }

    #[test]
    fn expand_string_errors_on_missing_env() {
        std::env::remove_var("LAZYCMAKE_EXPAND_MISSING_ABC");
        let err = expand_string(
            "$env{LAZYCMAKE_EXPAND_MISSING_ABC}/x",
            Path::new("/proj"),
            "p",
        )
        .unwrap_err();
        assert!(err.to_string().contains("LAZYCMAKE_EXPAND_MISSING_ABC"));
    }

    #[test]
    fn resolve_binary_dir_joins_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{
  "version": 6,
  "configurePresets": [
    { "name": "p", "binaryDir": "build-rel" }
  ]
}"#;
        fs::write(dir.path().join("CMakePresets.json"), json).unwrap();
        let store = PresetStore::load(dir.path()).unwrap();
        let preset = store.get("p").unwrap();
        assert_eq!(
            store.resolve_binary_dir(preset),
            dir.path().join("build-rel")
        );
    }
}
