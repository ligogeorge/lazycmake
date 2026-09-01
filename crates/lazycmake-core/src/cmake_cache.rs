use std::collections::HashMap;
use std::path::Path;

use crate::config::PresetOverrideConfig;
use crate::error::{Error, Result};
use crate::presets::{expand_string, ResolvedConfigurePreset};

/// Variables that identify which configure preset produced a build directory.
/// Inherited boilerplate (`CMAKE_*_WORKS`, `FETCHCONTENT_BASE_DIR`, …) is omitted
/// because CMake often drops or normalizes those entries in `CMakeCache.txt`.
const PRESET_IDENTITY_KEYS: &[&str] = &[
    "TRV_VERSION",
    "TRV_REVISION",
    "WITHOUT_LAB",
    "BOARD",
    "BOARD_ROOT",
    "WITHOUT_FW",
    "ENABLE_MUTATION_TESTING",
];

/// Effective `-D` cache variables for a configure (preset + optional override).
pub fn effective_configure_cache_variables(
    preset: &ResolvedConfigurePreset,
    override_cfg: Option<&PresetOverrideConfig>,
    project_root: &Path,
) -> Result<HashMap<String, String>> {
    let Some(ov) = override_cfg else {
        return Ok(preset.cache_variables.clone());
    };

    if ov.source_dir.is_none() && ov.generator.is_none() && ov.cache_variables.is_empty() {
        return Ok(preset.cache_variables.clone());
    }

    let mut vars = preset.cache_variables.clone();
    for (k, v) in &ov.cache_variables {
        vars.insert(k.clone(), expand_string(v, project_root, &preset.name)?);
    }
    if !vars.contains_key("CMAKE_BUILD_TYPE") {
        vars.insert("CMAKE_BUILD_TYPE".into(), "Debug".into());
    }
    Ok(vars)
}

pub fn read_cmake_cache(binary_dir: &Path) -> Result<HashMap<String, String>> {
    let path = binary_dir.join("CMakeCache.txt");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| Error::Cmake(e.to_string()))?;
    Ok(parse_cmake_cache(&text))
}

fn parse_cmake_cache(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        if line.starts_with("//") || line.is_empty() {
            continue;
        }
        let Some((key_part, value)) = line.split_once('=') else {
            continue;
        };
        let key = key_part.split(':').next().unwrap_or(key_part);
        out.insert(key.to_string(), value.to_string());
    }
    out
}

fn cache_value_matches(expected: &str, actual: &str) -> bool {
    if cache_bool(expected).is_some() || cache_bool(actual).is_some() {
        return cache_bool(expected) == cache_bool(actual);
    }
    expected == actual
}

fn cache_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn expected_bool(vars: &HashMap<String, String>, key: &str) -> bool {
    vars.get(key)
        .and_then(|v| cache_bool(v))
        .unwrap_or(false)
}

fn cache_bool_value(cache: &HashMap<String, String>, key: &str) -> bool {
    cache
        .get(key)
        .and_then(|v| cache_bool(v))
        .unwrap_or(false)
}

/// True when `binary_dir/CMakeCache.txt` matches the selected preset's cache variables.
pub fn cmake_cache_matches_preset(
    binary_dir: &Path,
    preset: &ResolvedConfigurePreset,
    override_cfg: Option<&PresetOverrideConfig>,
    project_root: &Path,
) -> Result<bool> {
    let cache = read_cmake_cache(binary_dir)?;
    if cache.is_empty() {
        return Ok(false);
    }

    let expected =
        effective_configure_cache_variables(preset, override_cfg, project_root)?;

    let mut keys: Vec<String> = PRESET_IDENTITY_KEYS
        .iter()
        .copied()
        .filter(|k| expected.contains_key(*k))
        .map(str::to_string)
        .collect();
    if let Some(ov) = override_cfg {
        for k in ov.cache_variables.keys() {
            if expected.contains_key(k) && !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }

    for key in keys {
        let Some(value) = expected.get(&key) else {
            continue;
        };
        let Some(actual) = cache.get(&key) else {
            if cache_bool(value) == Some(false) {
                continue;
            }
            return Ok(false);
        };
        if !cache_value_matches(value, actual) {
            return Ok(false);
        }
    }

    // Small presets set WITHOUT_LAB; Full presets do not. CMake keeps a stale
    // WITHOUT_LAB=TRUE in the shared build-nrf5 cache after switching families.
    let expected_lab = expected_bool(&expected, "WITHOUT_LAB");
    let cache_lab = cache_bool_value(&cache, "WITHOUT_LAB");
    if expected_lab != cache_lab {
        return Ok(false);
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn preset(name: &str, vars: &[(&str, &str)]) -> ResolvedConfigurePreset {
        ResolvedConfigurePreset {
            name: name.into(),
            display_name: None,
            description: None,
            generator: Some("Ninja".into()),
            binary_dir: PathBuf::from("build-nrf5"),
            cache_variables: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            hidden: false,
        }
    }

    #[test]
    fn parse_cmake_cache_reads_typed_entries() {
        let text = r#"
// comment
TRV_VERSION:UNINITIALIZED=7
WITHOUT_LAB:BOOL=TRUE
"#;
        let cache = parse_cmake_cache(text);
        assert_eq!(cache.get("TRV_VERSION").map(String::as_str), Some("7"));
        assert_eq!(cache.get("WITHOUT_LAB").map(String::as_str), Some("TRUE"));
    }

    #[test]
    fn matches_when_trv_vars_align() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CMakeCache.txt"),
            "TRV_VERSION:UNINITIALIZED=7\nTRV_REVISION:UNINITIALIZED=2\nWITHOUT_LAB:BOOL=TRUE\nWITHOUT_TESTS:BOOL=TRUE\n",
        )
        .unwrap();

        let p = preset(
            "TRV7-2Small",
            &[
                ("TRV_VERSION", "7"),
                ("TRV_REVISION", "2"),
                ("WITHOUT_LAB", "true"),
                ("WITHOUT_TESTS", "true"),
            ],
        );

        assert!(cmake_cache_matches_preset(dir.path(), &p, None, Path::new(".")).unwrap());
    }

    #[test]
    fn rejects_stale_trv_revision_from_another_preset() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CMakeCache.txt"),
            "TRV_VERSION:UNINITIALIZED=7\nTRV_REVISION:UNINITIALIZED=1\nWITHOUT_LAB:BOOL=TRUE\n",
        )
        .unwrap();

        let p = preset(
            "TRV7-2Small",
            &[
                ("TRV_VERSION", "7"),
                ("TRV_REVISION", "2"),
                ("WITHOUT_LAB", "true"),
            ],
        );

        assert!(!cmake_cache_matches_preset(dir.path(), &p, None, Path::new(".")).unwrap());
    }

    #[test]
    fn rejects_full_when_without_lab_stale_from_small_configure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CMakeCache.txt"),
            "TRV_VERSION:UNINITIALIZED=7\nTRV_REVISION:UNINITIALIZED=2\nWITHOUT_LAB:BOOL=TRUE\n",
        )
        .unwrap();

        let p = preset(
            "TRV7-2Full",
            &[("TRV_VERSION", "7"), ("TRV_REVISION", "2")],
        );

        assert!(!cmake_cache_matches_preset(dir.path(), &p, None, Path::new(".")).unwrap());
    }

    #[test]
    fn rejects_small_when_without_lab_missing_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CMakeCache.txt"),
            "TRV_VERSION:UNINITIALIZED=7\nTRV_REVISION:UNINITIALIZED=2\n",
        )
        .unwrap();

        let p = preset(
            "TRV7-2Small",
            &[
                ("TRV_VERSION", "7"),
                ("TRV_REVISION", "2"),
                ("WITHOUT_LAB", "true"),
            ],
        );

        assert!(!cmake_cache_matches_preset(dir.path(), &p, None, Path::new(".")).unwrap());
    }

    #[test]
    fn accepts_full_cache_for_full_preset() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CMakeCache.txt"),
            "TRV_VERSION:UNINITIALIZED=7\nTRV_REVISION:UNINITIALIZED=2\n",
        )
        .unwrap();

        let p = preset(
            "TRV7-2Full",
            &[("TRV_VERSION", "7"), ("TRV_REVISION", "2")],
        );

        assert!(cmake_cache_matches_preset(dir.path(), &p, None, Path::new(".")).unwrap());
    }
}
