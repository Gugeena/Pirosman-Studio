use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("timeline conflict: {0}")]
    Conflict(String),
    #[error("track is locked: {0}")]
    Locked(String),
    #[error("project file error: {0}")]
    Format(String),
    #[error("revision store integrity error: {0}")]
    Integrity(String),
    #[error("uncommitted changes: {0}")]
    Dirty(String),
    #[error("media tool error: {0}")]
    Media(String),
    #[error("bundle rejected: {0}")]
    Bundle(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl serde::Serialize for CoreError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
