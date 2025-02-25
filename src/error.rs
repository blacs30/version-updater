use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Failed to read config file: {0}")]
    FileRead(#[from] std::io::Error),
    #[error("Failed to parse config file: {0}")]
    ParseYaml(#[from] serde_yaml::Error),
    #[error("Missing project ID for GitLab repository")]
    MissingGitlabProjectId,
    #[error("Missing GitHub token for private repository or global auth")]
    MissingGithubToken,
    #[error("Missing GitLab token for private repository")]
    MissingGitlabToken,
    #[error("Missing Codeberg token for private repository")]
    MissingCodebergToken,
    #[error("Failed to read Docker credentials: {0}")]
    CredentialsError(String),

    #[error("Registry authentication failed: {0}")]
    AuthenticationError(String),

    #[error("Registry request failed: {0}")]
    RequestError(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Image not found: {0}")]
    ImageNotFound(String),

    #[error("Invalid registry response: {0}")]
    InvalidResponse(String),
}
