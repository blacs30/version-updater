use clap::{Parser, ValueEnum};
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
// use serde_json::json;
use regex::Regex;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use tokio;
const USER_AGENT_NAME: &str = "version-updater";
const DEFAULT_VERSION_FILTER: &str = "(.*)";

fn default_version_filter() -> String {
    DEFAULT_VERSION_FILTER.to_string()
}

impl fmt::Display for GitConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.project_id {
            Some(id) => write!(f, "{}", id),
            None => write!(f, "{}", self.repo),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct GitConfig {
    repo: String,
    #[serde(rename = "type")]
    git_type: GitType,
    #[serde(default)] // This makes it optional in the serialized form
    project_id: Option<u64>,
    #[serde(default = "default_version_filter", rename = "version_filter")]
    filter: String,
    #[serde(default)]
    private: bool,
}

impl GitConfig {
    // Validation method
    pub fn validate(&self) -> Result<(), String> {
        if self.git_type == GitType::Gitlab && self.project_id.is_none() {
            return Err("project_id is required when git type is gitlab".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum GitType {
    Github,
    Gitlab,
    None,
}
#[derive(Debug, Serialize, Deserialize)]
struct ImageConfig {
    registry: String,
    tag: String,
    #[serde(default)]
    private: bool,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Json)]
    format: OutputFormat,

    /// Config file path
    #[arg(short, long, default_value = "config.yaml")]
    config: String,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
enum OutputFormat {
    Json,
    Yaml,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    #[serde(flatten)]
    services: HashMap<String, ServiceConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServiceConfig {
    git: GitConfig,
    image: ImageConfig,
}

#[derive(Debug, Serialize, Deserialize)]
struct OutputData {
    version: HashMap<String, ServiceInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServiceInfo {
    container_image: String,
    image_tag: String,
}
async fn check_registry_image(
    registry: &str,
    tag_pattern: &str,
) -> Result<(String, String), Box<dyn Error>> {
    // This is a placeholder - implement actual registry checking logic
    Ok((registry.to_string(), tag_pattern.to_string()))
}

async fn get_github_version(
    repo: &str,
    token: Option<String>,
    filter: String,
) -> Result<String, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let mut request = client.get(url).header(USER_AGENT, USER_AGENT_NAME);

    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {}", token));
    }

    let response = request.send().await?;
    let body = response.text().await?;
    let data: serde_json::Value = serde_json::from_str(&body)?;
    let tag_name = data["tag_name"].as_str().unwrap_or("");

    let version_pattern = format!("{}", filter); // Just use the filter directly
    let re = Regex::new(&version_pattern).unwrap();
    let version = re
        .captures(tag_name)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    Ok(version)
}

async fn get_gitlab_version(repo: u64, token: Option<String>) -> Result<String, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://gitlab.com/api/v4/projects/{}/releases/permalink/latest",
        repo
    );
    let mut request = client.get(url).header(USER_AGENT, USER_AGENT_NAME);

    if let Some(token) = token {
        request = request.header("PRIVATE-TOKEN", format!("{}", token));
    }

    let response = request.send().await?;
    let body = response.text().await?;
    let data: serde_json::Value = serde_json::from_str(&body)?;
    println!("{:?}", data);
    Ok(data["tag_name"].as_str().unwrap_or("").to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let config_content = fs::read_to_string(&args.config)?;
    let config_file: Config = serde_yaml::from_str(&config_content)?;

    let mut output = OutputData {
        version: HashMap::new(),
    };

    // Process each service in the config
    for (service_name, service_config) in config_file.services {
        if let Err(e) = service_config.git.validate() {
            return Err(format!(
                "Invalid configuration for service '{}': {}",
                service_name, e
            )
            .into());
        }
        // Get version from git if configured
        let version = match service_config.git.git_type {
            GitType::Github => {
                get_github_version(
                    &service_config.git.repo,
                    if service_config.git.private {
                        env::var("GITHUB_TOKEN").ok()
                    } else {
                        None
                    },
                    service_config.git.filter,
                )
                .await?
            }
            GitType::Gitlab => {
                get_gitlab_version(
                    service_config.git.project_id.unwrap(),
                    if service_config.git.private {
                        env::var("GITLAB_TOKEN").ok()
                    } else {
                        None
                    },
                )
                .await?
            }
            GitType::None => String::new(),
        };
        println!("{}", version);

        // Check registry image
        let (container_image, mut image_tag) =
            check_registry_image(&service_config.image.registry, &service_config.image.tag).await?;

        // Replace ${RELEASE_VERSION} placeholder if present
        image_tag = image_tag.replace("${RELEASE_VERSION}", &version);

        // Add to output
        output.version.insert(
            service_name,
            ServiceInfo {
                container_image,
                image_tag,
            },
        );
    }

    // Output results in requested format
    match args.format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
        OutputFormat::Yaml => println!("{}", serde_yaml::to_string(&output)?),
    }

    Ok(())
}
