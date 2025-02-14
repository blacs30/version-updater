use super::error::VersionError;
use anyhow::Result;
use log::{debug, info, trace};
use regex::Regex;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;

const USER_AGENT_NAME: &str = "version-updater";
const DEFAULT_VERSION_FILTER: &str = "(.*)";

fn default_version_filter() -> String {
    DEFAULT_VERSION_FILTER.to_string()
}
pub struct GitClient {}
impl GitClient {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn get_version(&self, config: &Config) -> Result<String> {
        match config.git_type {
            Provider::Github => {
                self.get_github_version(
                    &config.repo,
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
                self.get_gitlab_version(
                    config.project_id.unwrap(),
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

    async fn get_github_version(
        &self,
        repo: &str,
        token: Option<String>,
        filter: &String,
    ) -> Result<String> {
        info!("Getting latest version from Github for repo {}", repo);
        let client = reqwest::Client::new();
        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
        debug!("Github query url {}", url);
        let mut request = client.get(url).header(USER_AGENT, USER_AGENT_NAME);
        trace!("Request for repo {} is {:?}", repo, request);

        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {}", token));
        }

        let response = request.send().await?;
        trace!("Response for repo {}is {:?}", repo, response);

        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status() == StatusCode::FORBIDDEN
        {
            return Err(VersionError::RateLimited("GitHub API".to_string()).into());
        }

        let body = response.text().await?;
        trace!("Body for repo {}is {:?}", repo, body);
        let data: serde_json::Value = serde_json::from_str(&body)?;
        trace!("Data for  repo {}is {:?}", repo, data);
        let tag_name = data["tag_name"].as_str().unwrap_or("");
        trace!("Tag for repo {}is {:?}", repo, tag_name);

        let version_pattern = filter.to_string();
        let re = Regex::new(&version_pattern).unwrap();
        let version = re
            .captures(tag_name)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if version.is_empty() {
            return Err(VersionError::NotFound(format!("No matching version for {}", repo)).into());
        }
        Ok(version)
    }

    async fn get_gitlab_version(
        &self,
        repo: u64,
        token: Option<String>,
        filter: &String,
    ) -> Result<String> {
        info!("Getting latest version from Gitlab for repo {}", repo);
        let client = reqwest::Client::new();
        let url = format!(
            "https://gitlab.com/api/v4/projects/{}/releases/permalink/latest",
            repo
        );
        debug!("Gitlab query url {}", url);
        let mut request = client.get(url).header(USER_AGENT, USER_AGENT_NAME);
        trace!("Request for repo {}is {:?}", repo, request);

        if let Some(token) = token {
            request = request.header("PRIVATE-TOKEN", token.to_string());
        }

        let response = request.send().await?;
        trace!("Response for repo {}is {:?}", repo, response);
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(VersionError::RateLimited("GitLab API".to_string()).into());
        }
        let body = response.text().await?;
        trace!("Body for repo {}is {:?}", repo, body);
        let data: serde_json::Value = serde_json::from_str(&body)?;
        trace!("Data for  repo {}is {:?}", repo, data);
        let tag_name = data["tag_name"].as_str().unwrap_or("");
        trace!("Tag for repo {}is {:?}", repo, tag_name);

        let version_pattern = filter.to_string();
        let re = Regex::new(&version_pattern).unwrap();
        let version = re
            .captures(tag_name)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if version.is_empty() {
            return Err(VersionError::NotFound(format!("No matching version for {}", repo)).into());
        }
        Ok(version)
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.project_id {
            Some(id) => write!(f, "{}", id),
            None => write!(f, "{}", self.repo),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
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

impl Config {
    // // Add a method to set the global authentication
    pub fn with_global_github_auth(mut self, auth: bool) -> Self {
        self.global_github_auth = auth;
        self
    }
    // Validation method
    pub fn validate(&self) -> Result<(), String> {
        if self.git_type == Provider::Gitlab && self.project_id.is_none() {
            return Err("project_id is required when git type is gitlab".to_string());
        }

        // Check tokens for private repositories or when global GitHub auth is enabled
        if self.private || (self.git_type == Provider::Github && self.global_github_auth) {
            match self.git_type {
                Provider::Github => {
                    if env::var("GITHUB_TOKEN").is_err() {
                        return Err("GITHUB_TOKEN environment variable is required for private Github repositories or when global authentication is enabled".to_string());
                    }
                }
                Provider::Gitlab => {
                    if env::var("GITLAB_TOKEN").is_err() {
                        return Err("GITLAB_TOKEN environment variable is required for private Gitlab repositories".to_string());
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
    None,
}
