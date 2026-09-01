//! Generic process env helpers (env files + temporary overlays).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

/// Source a shell script and return the exported environment (`env -0` after `source`).
pub fn load_env_file(path: &Path) -> Result<HashMap<String, String>> {
    if !path.is_file() {
        return Err(Error::Other(format!(
            "env_file does not exist: {}",
            path.display()
        )));
    }

    let output = Command::new("bash")
        .args([
            "-c",
            "set +u; source \"$1\"; set -u; env -0",
            "_",
            &path.display().to_string(),
        ])
        .output()
        .map_err(|e| Error::Other(format!("failed to source env_file: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Other(format!(
            "sourcing env_file {} failed: {stderr}",
            path.display()
        )));
    }

    Ok(parse_env_null(&output.stdout))
}

/// Apply `vars` until the guard is dropped (restores prior values).
pub struct EnvOverlay {
    previous: Vec<(String, Option<String>)>,
}

impl EnvOverlay {
    pub fn apply(vars: &HashMap<String, String>) -> Self {
        let mut previous = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            previous.push((key.clone(), std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        Self { previous }
    }
}

impl Drop for EnvOverlay {
    fn drop(&mut self) {
        for (key, old) in self.previous.drain(..) {
            match old {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

pub fn parse_env_null(bytes: &[u8]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entry in bytes.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        if key == "_" || key == "SHLVL" || key == "PWD" || key == "OLDPWD" {
            continue;
        }
        map.insert(key.to_string(), value.to_string());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_env_null_reads_pairs() {
        let raw = b"FOO=bar\0PATH=/a:/b\0_\0SHLVL=2\0";
        let map = parse_env_null(raw);
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("PATH").map(String::as_str), Some("/a:/b"));
        assert!(!map.contains_key("_"));
        assert!(!map.contains_key("SHLVL"));
    }

    #[test]
    fn env_overlay_restores_previous_values() {
        let key = "LAZYCMAKE_ENV_OVERLAY_TEST";
        std::env::remove_var(key);
        {
            let mut vars = HashMap::new();
            vars.insert(key.into(), "during".into());
            let _guard = EnvOverlay::apply(&vars);
            assert_eq!(std::env::var(key).unwrap(), "during");
        }
        assert!(std::env::var(key).is_err());
    }

    #[test]
    fn load_env_file_sources_exports() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extra.env.sh");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "export LAZYCMAKE_ENV_FILE_TEST=from_file").unwrap();
        let map = load_env_file(&path).unwrap();
        assert_eq!(
            map.get("LAZYCMAKE_ENV_FILE_TEST").map(String::as_str),
            Some("from_file")
        );
    }
}
