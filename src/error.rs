use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required configuration: {0}")]
    Missing(&'static str),
    #[error("invalid configuration for {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
}
