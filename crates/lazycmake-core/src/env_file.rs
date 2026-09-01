//! Generic process env helpers (env files + temporary overlays).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// Load environment variables from a file.
///
/// Prefers sourcing the file with bash (full shell scripts). If bash is
/// unavailable or fails, falls back to simple `KEY=VALUE` / `export KEY=VALUE`
/// lines (no command substitution).
pub fn load_env_file(path: &Path) -> Result<HashMap<String, String>> {
    if !path.is_file() {
        return Err(Error::Other(format!(
            "env_file does not exist: {}",
            path.display()
        )));
    }

    match load_env_file_via_bash(path) {
        Ok(map) => Ok(map),
        Err(bash_err) => load_env_file_simple(path).map_err(|simple_err| {
            Error::Other(format!(
                "env_file {}: bash failed ({bash_err}); simple parse failed ({simple_err})",
                path.display()
            ))
        }),
    }
}

fn load_env_file_via_bash(path: &Path) -> Result<HashMap<String, String>> {
    let bash = find_bash().ok_or_else(|| Error::Other("bash not found on PATH".into()))?;
    // Bash on Windows mishandles backslashes in paths (e.g. `\t` in `\Temp`).
    let path_arg = path_for_bash(path);

    let output = Command::new(&bash)
        .args([
            "-c",
            // Strip CRLF so scripts checked out / written on Windows still source cleanly.
            "set +u; tmp=$(mktemp); tr -d '\\r' < \"$1\" > \"$tmp\"; . \"$tmp\"; rm -f \"$tmp\"; set -u; env -0",
            "_",
            &path_arg,
        ])
        .output()
        .map_err(|e| Error::Other(format!("failed to spawn bash ({}): {e}", bash.display())))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(Error::Other(format!(
            "bash exited {}: stderr={stderr:?} stdout={stdout:?}",
            output.status.code().unwrap_or(-1)
        )));
    }

    Ok(parse_env_null(&output.stdout))
}

/// `KEY=VALUE` / `export KEY=VALUE` lines only (no shell expansion).
fn load_env_file_simple(path: &Path) -> Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)?;
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export ")
            .map(str::trim)
            .unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.contains(' ') {
            continue;
        }
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        map.insert(key.to_string(), value.to_string());
    }
    if map.is_empty() {
        return Err(Error::Other(
            "no KEY=VALUE assignments found (bash required for shell scripts)".into(),
        ));
    }
    Ok(map)
}

fn path_for_bash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn find_bash() -> Option<PathBuf> {
    if which_bash_on_path().is_ok() {
        return Some(PathBuf::from("bash"));
    }
    #[cfg(windows)]
    {
        const CANDIDATES: &[&str] = &[
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ];
        for candidate in CANDIDATES {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn which_bash_on_path() -> std::io::Result<()> {
    Command::new("bash")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| ())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "bash"))
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
        // Explicit `\n` so Windows runners don't rely on CRLF-only scripts.
        write!(f, "export LAZYCMAKE_ENV_FILE_TEST=from_file\n").unwrap();
        let map = load_env_file(&path).unwrap();
        assert_eq!(
            map.get("LAZYCMAKE_ENV_FILE_TEST").map(String::as_str),
            Some("from_file")
        );
    }

    #[test]
    fn load_env_file_simple_parses_export_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("simple.env");
        std::fs::write(&path, "export A=1\nB=two\n# comment\n\n").unwrap();
        let map = load_env_file_simple(&path).unwrap();
        assert_eq!(map.get("A").map(String::as_str), Some("1"));
        assert_eq!(map.get("B").map(String::as_str), Some("two"));
    }
}
