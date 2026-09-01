use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

use crate::cmake::Generator;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Executable,
    Library,
    Utility,
    Other,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Executable => "exe",
            Self::Library => "lib",
            Self::Utility => "utl",
            Self::Other => "oth",
        }
    }

    fn from_type_name(type_name: &str) -> Self {
        match type_name {
            "EXECUTABLE" => Self::Executable,
            "STATIC_LIBRARY" | "SHARED_LIBRARY" | "MODULE_LIBRARY" | "OBJECT_LIBRARY"
            | "INTERFACE_LIBRARY" => Self::Library,
            "UTILITY" => Self::Utility,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodemodelTarget {
    pub name: String,
    pub kind: TargetKind,
    pub paths: Vec<PathBuf>,
}

pub fn ensure_codemodel_query(binary_dir: &Path) -> Result<()> {
    let query_dir = binary_dir.join(".cmake/api/v1/query/client-lazycmake");
    fs::create_dir_all(&query_dir)?;
    // Stateless File API query: empty file named `<kind>-v<major>`.
    // Do not use query.json with kind "codemodel-v2" — cmake rejects that kind
    // (`unknown request kind`); the kind is "codemodel", version major 2.
    fs::write(query_dir.join("codemodel-v2"), "")?;
    // Remove a previously written broken query.json so it cannot poison replies.
    let _ = fs::remove_file(query_dir.join("query.json"));
    Ok(())
}

/// Re-run cmake on an existing build tree so File API replies are generated.
///
/// Stdio is discarded: this may run on the TUI thread and must not paint the tty.
pub fn regenerate_codemodel(binary_dir: &Path) -> Result<()> {
    ensure_codemodel_query(binary_dir)?;
    let status = Command::new("cmake")
        .arg(binary_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| Error::Cmake(e.to_string()))?;
    if !status.success() {
        return Err(Error::Cmake(format!(
            "cmake {} failed (exit {})",
            binary_dir.display(),
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

pub fn load_targets(binary_dir: &Path, generator: Generator) -> Result<Vec<CodemodelTarget>> {
    let reply_root = binary_dir.join(".cmake/api/v1/reply");
    if !reply_root.exists() {
        return Ok(Vec::new());
    }

    let codemodel_path = find_codemodel_file(&reply_root)?;
    let Some(path) = codemodel_path else {
        return Ok(Vec::new());
    };

    let contents = fs::read_to_string(&path)?;
    let mut targets = parse_codemodel(&contents, &reply_root, generator)?;
    ensure_default_build_target(&mut targets);
    Ok(targets)
}

/// CMake File API never reports the implicit `all` target (what `cmake --build`
/// / `west build` / plain `ninja` builds). Insert it at the front when missing.
fn ensure_default_build_target(targets: &mut Vec<CodemodelTarget>) {
    if targets.iter().any(|t| t.name == "all") {
        return;
    }
    targets.insert(
        0,
        CodemodelTarget {
            name: "all".into(),
            kind: TargetKind::Utility,
            paths: Vec::new(),
        },
    );
}

fn find_codemodel_file(reply_root: &Path) -> Result<Option<PathBuf>> {
    if let Some(path) = codemodel_from_index(reply_root)? {
        return Ok(Some(path));
    }

    let mut reply_files: Vec<PathBuf> = fs::read_dir(reply_root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "json")
                && p.file_name().is_some_and(|n| {
                    let name = n.to_string_lossy();
                    name.starts_with("codemodel-v2-") || name == "codemodel-v2.json"
                })
        })
        .collect();
    reply_files.sort();
    Ok(reply_files.last().cloned())
}

fn codemodel_from_index(reply_root: &Path) -> Result<Option<PathBuf>> {
    let mut index_files: Vec<PathBuf> = fs::read_dir(reply_root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("index-")))
        .collect();
    index_files.sort();
    let Some(index_path) = index_files.last() else {
        return Ok(None);
    };

    let index: Value = serde_json::from_str(&fs::read_to_string(index_path)?)?;
    let reply = index.get("reply").and_then(Value::as_object);
    let Some(reply) = reply else {
        return Ok(None);
    };

    for client in reply.values() {
        if let Some(json_file) = client
            .get("query.json")
            .and_then(|q| q.get("codemodel-v2"))
            .and_then(|c| c.get("jsonFile"))
            .and_then(Value::as_str)
        {
            return Ok(Some(reply_root.join(json_file)));
        }
        if let Some(json_file) = client
            .get("codemodel-v2")
            .and_then(|c| c.get("jsonFile"))
            .and_then(Value::as_str)
        {
            return Ok(Some(reply_root.join(json_file)));
        }
    }

    Ok(None)
}

fn parse_codemodel(contents: &str, reply_root: &Path, generator: Generator) -> Result<Vec<CodemodelTarget>> {
    let model: CodemodelReply = serde_json::from_str(contents).map_err(|e| Error::Parse(e.to_string()))?;
    let mut targets = Vec::new();

    for config in model.configurations {
        for target in config.targets {
            if let Some(json_file) = target.json_file {
                if let Ok(detail) = load_target_detail(&reply_root.join(json_file)) {
                    targets.push(detail);
                    continue;
                }
            }

            if let Some(type_name) = target.r#type {
                let mut paths = Vec::new();
                if let Some(artifacts) = target.artifacts {
                    for artifact in artifacts {
                        if let Some(path) = artifact.path {
                            paths.push(PathBuf::from(path));
                        }
                    }
                }
                targets.push(CodemodelTarget {
                    name: target.name,
                    kind: TargetKind::from_type_name(&type_name),
                    paths,
                });
            } else {
                targets.push(CodemodelTarget {
                    name: target.name,
                    kind: TargetKind::Other,
                    paths: Vec::new(),
                });
            }
        }
    }

    targets.sort_by(|a, b| a.name.cmp(&b.name));
    let _ = generator;
    Ok(targets)
}

fn load_target_detail(path: &Path) -> Result<CodemodelTarget> {
    let detail: TargetDetailJson = serde_json::from_str(&fs::read_to_string(path)?)
        .map_err(|e| Error::Parse(e.to_string()))?;
    let paths = detail
        .artifacts
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| a.path.map(PathBuf::from))
        .collect();
    Ok(CodemodelTarget {
        name: detail.name,
        kind: TargetKind::from_type_name(&detail.r#type),
        paths,
    })
}

pub fn executable_path(target: &CodemodelTarget, binary_dir: &Path, generator: Generator) -> Option<PathBuf> {
    if target.kind != TargetKind::Executable {
        return None;
    }
    if let Some(path) = target.paths.first() {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            binary_dir.join(path)
        };
        return Some(with_exe_suffix(path, generator));
    }
    Some(with_exe_suffix(binary_dir.join(&target.name), generator))
}

fn with_exe_suffix(path: PathBuf, generator: Generator) -> PathBuf {
    let suffix = generator.exe_suffix();
    if suffix.is_empty() || path.extension().is_some() {
        path
    } else {
        PathBuf::from(format!("{}{}", path.display(), suffix))
    }
}

#[derive(Debug, Deserialize)]
struct CodemodelReply {
    #[serde(default)]
    configurations: Vec<CodemodelConfiguration>,
}

#[derive(Debug, Deserialize)]
struct CodemodelConfiguration {
    #[serde(default)]
    targets: Vec<CodemodelTargetRef>,
}

#[derive(Debug, Deserialize)]
struct CodemodelTargetRef {
    name: String,
    #[serde(rename = "type")]
    r#type: Option<String>,
    #[serde(rename = "jsonFile")]
    json_file: Option<String>,
    #[serde(default)]
    artifacts: Option<Vec<CodemodelArtifact>>,
}

#[derive(Debug, Deserialize)]
struct CodemodelArtifact {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TargetDetailJson {
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    #[serde(default)]
    artifacts: Option<Vec<CodemodelArtifact>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codemodel_fixture() {
        let json = include_str!("../tests/fixtures/codemodel-v2.json");
        let dir = tempfile::tempdir().unwrap();
        let targets = parse_codemodel(json, dir.path(), Generator::Ninja).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "hello");
        assert_eq!(targets[0].kind, TargetKind::Executable);
    }

    #[test]
    fn parses_codemodel_with_target_refs() {
        let dir = tempfile::tempdir().unwrap();
        let reply = dir.path();
        fs::write(
            reply.join("target-hello.json"),
            r#"{"name":"hello","type":"EXECUTABLE","artifacts":[{"path":"hello"}]}"#,
        )
        .unwrap();
        let codemodel = r#"{"configurations":[{"targets":[{"name":"hello","jsonFile":"target-hello.json"}]}]}"#;
        let targets = parse_codemodel(codemodel, reply, Generator::Ninja).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, TargetKind::Executable);
    }

    #[test]
    fn parses_empty_codemodel() {
        let json = r#"{"configurations":[]}"#;
        let dir = tempfile::tempdir().unwrap();
        let targets = parse_codemodel(json, dir.path(), Generator::Ninja).unwrap();
        assert!(targets.is_empty());
    }

    #[test]
    fn ensure_default_build_target_prepends_all_once() {
        let mut targets = vec![CodemodelTarget {
            name: "tracker-application".into(),
            kind: TargetKind::Utility,
            paths: Vec::new(),
        }];
        ensure_default_build_target(&mut targets);
        assert_eq!(targets[0].name, "all");
        assert_eq!(targets[0].kind, TargetKind::Utility);
        assert_eq!(targets[1].name, "tracker-application");
        ensure_default_build_target(&mut targets);
        assert_eq!(targets.iter().filter(|t| t.name == "all").count(), 1);
    }

    #[test]
    fn load_targets_prepends_synthetic_all() {
        let dir = tempfile::tempdir().unwrap();
        let reply = dir.path().join(".cmake/api/v1/reply");
        fs::create_dir_all(&reply).unwrap();
        let codemodel = reply.join("codemodel-v2-test.json");
        fs::write(
            &codemodel,
            r#"{"configurations":[{"targets":[{"name":"tracker-application","type":"UTILITY"}]}]}"#,
        )
        .unwrap();
        fs::write(
            reply.join("index-test.json"),
            format!(
                r#"{{"reply":{{"client-lazycmake":{{"codemodel-v2":{{"jsonFile":"{}"}}}}}}}}"#,
                codemodel.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();

        let targets = load_targets(dir.path(), Generator::Ninja).unwrap();
        assert_eq!(targets[0].name, "all");
        assert!(targets.iter().any(|t| t.name == "tracker-application"));
    }

    #[test]
    fn target_kind_labels() {
        assert_eq!(TargetKind::Executable.label(), "exe");
        assert_eq!(TargetKind::Library.label(), "lib");
        assert_eq!(TargetKind::Utility.label(), "utl");
        assert_eq!(TargetKind::Other.label(), "oth");
        assert_eq!(TargetKind::from_type_name("STATIC_LIBRARY"), TargetKind::Library);
        assert_eq!(TargetKind::from_type_name("UTILITY"), TargetKind::Utility);
    }

    #[test]
    fn executable_path_uses_artifact_and_ignores_libraries() {
        let binary_dir = Path::new("/build");
        let exe = CodemodelTarget {
            name: "hello".into(),
            kind: TargetKind::Executable,
            paths: vec![PathBuf::from("bin/hello")],
        };
        let expected = binary_dir.join("bin/hello");
        let expected = if cfg!(windows) {
            PathBuf::from(format!("{}.exe", expected.display()))
        } else {
            expected
        };
        assert_eq!(
            executable_path(&exe, binary_dir, Generator::Ninja),
            Some(expected)
        );

        let lib = CodemodelTarget {
            name: "libx".into(),
            kind: TargetKind::Library,
            paths: vec![PathBuf::from("libx.a")],
        };
        assert!(executable_path(&lib, binary_dir, Generator::Ninja).is_none());
    }

    #[test]
    fn ensure_codemodel_query_writes_stateless_codemodel_v2_file() {
        let dir = tempfile::tempdir().unwrap();
        let client = dir.path().join(".cmake/api/v1/query/client-lazycmake");
        fs::create_dir_all(&client).unwrap();
        fs::write(
            client.join("query.json"),
            r#"{"requests":[{"kind":"codemodel-v2","version":2}]}"#,
        )
        .unwrap();

        ensure_codemodel_query(dir.path()).unwrap();

        assert!(client.join("codemodel-v2").is_file());
        assert!(!client.join("query.json").exists());
    }
}
