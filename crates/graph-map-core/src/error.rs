use std::fmt;

pub type Result<T> = std::result::Result<T, GraphError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphError {
    pub status: u16,
    pub message: String,
}

impl GraphError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: 403,
            message: message.into(),
        }
    }

    pub fn missing(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: 409,
            message: message.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: 503,
            message: message.into(),
        }
    }
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GraphError {}

impl From<std::io::Error> for GraphError {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::missing(error.to_string()),
            std::io::ErrorKind::PermissionDenied => Self::forbidden(error.to_string()),
            _ => Self::unavailable(error.to_string()),
        }
    }
}

impl From<serde_json::Error> for GraphError {
    fn from(error: serde_json::Error) -> Self {
        Self::invalid(format!("Invalid JSON: {error}"))
    }
}
