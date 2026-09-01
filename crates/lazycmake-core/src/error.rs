use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("project not found: no CMakePresets.json or CMakeLists.txt in {0}")]
    ProjectNotFound(String),
    #[error("preset not found: {0}")]
    PresetNotFound(String),
    #[error("environment variable not set: {0}")]
    EnvVarMissing(String),
    #[error("cmake error: {0}")]
    Cmake(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("{0}")]
    Other(String),
}
