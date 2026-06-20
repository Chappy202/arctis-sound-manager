use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error on {path}: {source_msg}")]
    Io { path: String, source_msg: String },
    #[error("failed to parse config: {0}")]
    Parse(String),
    #[error("failed to serialize config: {0}")]
    Serialize(String),
    #[error("unsupported config version {found}; max supported is {max}")]
    UnsupportedVersion { found: u32, max: u32 },
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("invalid config: {0}")]
    Invalid(String),
}
