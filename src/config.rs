use super::error::ConfigError;
use super::git::Config as GitConfig;
use super::registry::ImageConfig;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use log::{debug, error, info, trace};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;

#[derive(Debug)]
pub struct AppConfig {
    pub args: Args,
    pub services: HashMap<String, ServiceConfig>,
}

impl AppConfig {
    pub fn load_config() -> Result<Self, ConfigError> {
        // Parse command line arguments
        let args = Args::parse();
        info!("Reading config file: {}", args.config);

        // Read and parse the config file
        let config_content = fs::read_to_string(&args.config)?;
        debug!("Config content read");

        // Parse YAML into Config struct
        let mut config: Config = serde_yaml::from_str(&config_content)?;
        trace!("Config content is {}", config_content);

        // Create a new HashMap to store the updated services
        let mut updated_services = HashMap::new();

        for (name, service) in config.services.iter_mut() {
            service.git = <GitConfig as Clone>::clone(&service.git)
                .with_global_github_auth(config.global.git.github.authenticate);

            match service.git.validate() {
                Ok(()) => {
                    updated_services.insert(name.clone(), service.clone());
                }
                Err(ConfigError::MissingGitlabProjectId) => {
                    error!("Service '{}' is missing GitLab project ID", name);
                    return Err(ConfigError::MissingGitlabProjectId);
                }
                Err(ConfigError::MissingGithubToken) => {
                    error!(
                        "Service '{}' requires GitHub token for authentication",
                        name
                    );
                    return Err(ConfigError::MissingGithubToken);
                }
                Err(ConfigError::MissingGitlabToken) => {
                    error!(
                        "Service '{}' requires GitLab token for authentication",
                        name
                    );
                    return Err(ConfigError::MissingGitlabToken);
                }
                Err(e) => {
                    error!("Invalid configuration for service '{}': {}", name, e);
                    return Err(e);
                }
            }
        }

        Ok(Self {
            args,
            services: updated_services,
        })
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    /// Config file path
    #[arg(short, long, default_value = "config.yaml")]
    pub config: String,

    /// Output file path
    #[arg(short = 'o', long, required = true)]
    pub output: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(transparent)]
pub struct OutputData {
    pub versions: BTreeMap<String, ServiceInfo>,
}

impl OutputData {
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
        }
    }
    // Optional: Add a convenience method to add services
    pub fn add_service(&mut self, name: String, info: ServiceInfo) {
        self.versions.insert(name, info);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub container_image: String,
    pub image_tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ServiceInfo {
    pub fn error(image: String, error: &str) -> Self {
        Self {
            container_image: image,
            image_tag: "<ERROR>".to_string(),
            error: Some(error.to_string()),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
pub enum OutputFormat {
    Json,
    Yaml,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub global: GlobalConfig,
    pub services: HashMap<String, ServiceConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GlobalConfig {
    pub git: GlobalGitConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GlobalGitConfig {
    pub github: GlobalGithubConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
/// Set this to true and provide a GITHUB_TOKEN env variable
/// to make authenticated GitHub API requests to avoid rate limiting (higher amount of API requests are allowed)
pub struct GlobalGithubConfig {
    pub authenticate: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServiceConfig {
    pub git: GitConfig,
    pub image: ImageConfig,
}
