use super::error::AppError;
use anyhow::Result;
use log::{debug, error, info, trace};
use regex::Regex;
use reqwest::header::USER_AGENT;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;

pub const USER_AGENT_NAME: &str = "version-updater";
const DEFAULT_VERSION_FILTER: &str = "(.*)";

fn default_version_filter() -> String {
    DEFAULT_VERSION_FILTER.to_string()
}
pub struct GitClient;

impl GitClient {
    pub async fn get_version(config: &GitConfig) -> Result<String> {
        match config.git_type {
            Provider::Codeberg => {
                Self::get_version_from_api(
                    ApiType::Codeberg { repo: &config.repo },
                    if config.private || config.global_github_auth {
                        env::var("CODEBERG_TOKEN").ok()
                    } else {
                        None
                    },
                    &config.filter,
                )
                .await
            }
            Provider::Github => {
                Self::get_version_from_api(
                    ApiType::Github { repo: &config.repo },
                    if config.private || config.global_github_auth {
                        env::var("GITHUB_TOKEN").ok()
                    } else {
                        None
                    },
                    &config.filter,
                )
                .await
            }
            Provider::Gitlab => {
                Self::get_version_from_api(
                    ApiType::Gitlab {
                        project_id: config.project_id.unwrap(),
                    },
                    if config.private {
                        env::var("GITLAB_TOKEN").ok()
                    } else {
                        None
                    },
                    &config.filter,
                )
                .await
            }
            Provider::None => Ok(String::new()),
        }
    }

    async fn get_version_from_api(
        api_type: ApiType<'_>,
        token: Option<String>,
        filter: &str,
    ) -> Result<String> {
        let (url, auth_header) = api_type.get_request_details(token);
        info!("Getting latest version from {} for {}", api_type, url);
        debug!("API query url {}", url);

        let client = reqwest::Client::new();
        let mut request = client.get(url).header(USER_AGENT, USER_AGENT_NAME);

        if let Some((header_name, header_value)) = auth_header {
            request = request.header(header_name, header_value);
        }

        trace!("Request is {:?}", request);
        let response = request.send().await?;
        trace!("Response is {:?}", response);

        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status() == StatusCode::FORBIDDEN
        {
            error!("{}: Failed to get version: Rate limited", api_type);
            return Err(AppError::RateLimited(format!("{} API", api_type)).into());
        }

        let body = response.text().await?;
        trace!("Body is {:?}", body);
        let data: serde_json::Value = serde_json::from_str(&body)?;
        trace!("Data is {:?}", data);

        let tag_name = data["tag_name"].as_str().unwrap_or("");
        trace!("Tag is {:?}", tag_name);

        extract_version(tag_name, filter, api_type)
    }
}

impl fmt::Display for GitConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.project_id {
            Some(id) => write!(f, "{}", id),
            None => write!(f, "{}", self.repo),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GitConfig {
    pub repo: String,
    #[serde(rename = "type")]
    pub git_type: Provider,
    #[serde(default)] // This makes it optional in the serialized form
    pub project_id: Option<u64>,
    #[serde(default = "default_version_filter", rename = "version_filter")]
    pub filter: String,
    #[serde(default)]
    pub private: bool,
    #[serde(skip)]
    pub global_github_auth: bool,
}

impl GitConfig {
    // Add a method to set the global authentication
    pub fn with_global_github_auth(mut self, auth: bool) -> Self {
        self.global_github_auth = auth;
        self
    }
    // Validation method
    pub fn validate(&self) -> Result<(), AppError> {
        if self.git_type == Provider::Gitlab && self.project_id.is_none() {
            return Err(AppError::MissingGitlabProjectId);
        }

        if self.private || (self.git_type == Provider::Github && self.global_github_auth) {
            match self.git_type {
                Provider::Github => {
                    if env::var("GITHUB_TOKEN").is_err() {
                        return Err(AppError::MissingGithubToken);
                    }
                }
                Provider::Gitlab => {
                    if env::var("GITLAB_TOKEN").is_err() {
                        return Err(AppError::MissingGitlabToken);
                    }
                }
                Provider::Codeberg => {
                    if env::var("CODEBERG_TOKEN").is_err() {
                        return Err(AppError::MissingCodebergToken);
                    }
                }
                Provider::None => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
    Codeberg,
    None,
}

enum ApiType<'a> {
    Github { repo: &'a str },
    Codeberg { repo: &'a str },
    Gitlab { project_id: u64 },
}

impl ApiType<'_> {
    fn get_request_details(&self, token: Option<String>) -> (String, Option<(String, String)>) {
        match self {
            ApiType::Codeberg { repo } => (
                format!("https://codeberg.org/api/v1/repos/{}/releases/latest", repo),
                token.map(|t| ("Authorization".to_string(), format!("Bearer {}", t))),
            ),
            ApiType::Github { repo } => (
                format!("https://api.github.com/repos/{}/releases/latest", repo),
                token.map(|t| ("Authorization".to_string(), format!("Bearer {}", t))),
            ),
            ApiType::Gitlab { project_id } => (
                format!(
                    "https://gitlab.com/api/v4/projects/{}/releases/permalink/latest",
                    project_id
                ),
                token.map(|t| ("PRIVATE-TOKEN".to_string(), t)),
            ),
        }
    }
}

impl fmt::Display for ApiType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiType::Github { repo } => write!(f, "GitHub({})", repo),
            ApiType::Codeberg { repo } => write!(f, "Codeberg({})", repo),
            ApiType::Gitlab { project_id } => write!(f, "GitLab({})", project_id),
        }
    }
}

fn extract_version(tag_name: &str, filter: &str, api_type: ApiType<'_>) -> Result<String> {
    let re = Regex::new(filter).unwrap();
    let version = re
        .captures(tag_name)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    if version.is_empty() {
        error!("No matching version for {}", api_type);
        return Err(AppError::NotFound(format!("No matching version for {}", api_type)).into());
    }
    Ok(version)
}
