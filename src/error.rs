use std::fmt;

#[derive(Debug)]
pub enum VersionError {
    RateLimited(String),
    NotFound(String),
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited(msg) => write!(f, "Rate limited: {}", msg),
            Self::NotFound(msg) => write!(f, "Not found: {}", msg),
        }
    }
}

impl std::error::Error for VersionError {}
