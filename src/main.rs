use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::{Parser, ValueEnum};
use env_logger::Builder;
use log::{debug, error, info, trace, warn};
use regex::Regex;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Write;
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

        // Check tokens for private repositories
        if self.private {
            match self.git_type {
                GitType::Github => {
                    if env::var("GITHUB_TOKEN").is_err() {
                        return Err("GITHUB_TOKEN environment variable is required for private Github repositories".to_string());
                    }
                }
                GitType::Gitlab => {
                    if env::var("GITLAB_TOKEN").is_err() {
                        return Err("GITLAB_TOKEN environment variable is required for private Gitlab repositories".to_string());
                    }
                }
                GitType::None => {}
            }
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
    name: String,
    tag: String,
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

    /// Output file path
    #[arg(short = 'o', long, required = true)]
    output: String,
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
    versions: HashMap<String, ServiceInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServiceInfo {
    container_image: String,
    image_tag: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: String,
}

#[derive(Deserialize)]
struct DockerConfig {
    auths: std::collections::HashMap<String, DockerAuth>,
}

#[derive(Deserialize)]
struct DockerAuth {
    auth: Option<String>,
    username: Option<String>,
    password: Option<String>,
}
enum RegistryAuth {
    DockerHub {
        auth_url: String,
        service: String,
    },
    GitLab {
        auth_url: String,
        service: String,
        client_id: String,
    },
    Generic {
        auth_url: String,
        service: String,
    },
}
impl RegistryAuth {
    fn from_registry(registry: &str) -> Self {
        match registry {
            "registry.hub.docker.com" => RegistryAuth::DockerHub {
                auth_url: "https://auth.docker.io/token".to_string(),
                service: "registry.docker.io".to_string(),
            },
            r if r.contains("gitlab") => RegistryAuth::GitLab {
                auth_url: "https://gitlab.com/jwt/auth".to_string(),
                service: "container_registry".to_string(),
                client_id: "docker".to_string(),
            },
            _ => RegistryAuth::Generic {
                auth_url: format!("https://{}/v2/token", registry),
                service: registry.to_string(),
            },
        }
    }
}

fn get_docker_credentials(registry: &str) -> Option<(String, String)> {
    info!("Gettinig docker credentials for {}", registry);
    let config_path = dirs::home_dir()?.join(".docker/config.json");
    trace!("Trying to read docker credentials from ~/.docker/config.json");
    let config_contents = fs::read_to_string(config_path).ok()?;
    let config: DockerConfig = serde_json::from_str(&config_contents).ok()?;

    let auth = config.auths.get(registry)?;

    // If we have a base64-encoded auth string, decode it
    if let Some(auth_str) = &auth.auth {
        info!("Found docker credentials base64 encoded for {}", registry);
        let decoded = String::from_utf8(STANDARD.decode(&auth_str).ok()?).ok()?;
        let mut parts = decoded.splitn(2, ':');
        let username = parts.next()?;
        let password = parts.next()?;
        return Some((username.to_string(), password.to_string()));
    }

    // Otherwise try to use explicit username/password
    if let (Some(username), Some(password)) = (&auth.username, &auth.password) {
        info!(
            "Found docker credentials username/password for {}",
            registry
        );
        return Some((username.clone(), password.clone()));
    }

    info!("No docker credentials found for {}", registry);
    None
}

async fn check_manifest(client: &Client, manifest_url: &str, token: Option<&str>) -> Result<bool> {
    info!("Getting image manifest at URL: {}", manifest_url);
    let accept_headers = [
        "application/vnd.docker.distribution.manifest.v2+json",
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
    ];

    for accept in accept_headers {
        debug!("Trying manifest format: {}", accept);

        let mut request = client.get(manifest_url).header("Accept", accept);

        // Only add authorization header if token is present
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = request.send().await?;

        match response.status() {
            StatusCode::OK => {
                info!("Successfully found manifest with format: {}", accept);
                return Ok(true);
            }
            StatusCode::NOT_FOUND => {
                if let Ok(error_body) = response.text().await {
                    warn!("Manifest not found with format: {}", accept);
                    debug!(
                        "Got 404 with for format {} with error body: {}",
                        accept, error_body
                    );
                    if error_body.contains("OCI index found")
                        || error_body.contains("manifest unknown")
                        || error_body.contains("MANIFEST_UNKNOWN")
                    {
                        continue;
                    }
                }
                if accept == accept_headers[accept_headers.len() - 1] {
                    return Ok(false);
                }
            }
            StatusCode::TOO_MANY_REQUESTS => {
                error!("Rate limit hit for: {}", manifest_url);
                return Ok(false);
            }
            status => {
                warn!("Unexpected status code: {} when checking manifest", status);
                if let Ok(error_body) = response.text().await {
                    return Err(anyhow!(
                        "Unexpected status code: {} with body: {}",
                        status,
                        error_body
                    ));
                } else {
                    return Err(anyhow!("Unexpected status code: {}", status));
                }
            }
        }
    }

    error!(
        "No manifest found or no accept header was correct for URL {}",
        manifest_url
    );
    Ok(false)
}

async fn get_registry_token(
    client: &Client,
    registry: &str,
    image_name: &str,
    creds: Option<(String, String)>,
) -> Result<Option<String>> {
    // Skip authentication for quay.io
    if registry.contains("quay.io") {
        info!("quay.io doesn't need API keys to access the API at the moment.");
        return Ok(None);
    } else {
        info!("Getting registry token for {}", registry);
    }

    let auth = RegistryAuth::from_registry(registry);

    // Get token based on registry type
    let token = match auth {
        RegistryAuth::DockerHub { auth_url, service } => {
            let token_url = format!(
                "{}?service={}&scope=repository:{}:pull",
                auth_url, service, image_name
            );
            trace!("Registry token_url is {}", token_url);

            let mut token_request = client.get(&token_url);
            if let Some((username, password)) = creds {
                token_request = token_request.basic_auth(username, Some(password));
            }

            let response = token_request.send().await?;
            let body = response.text().await?;
            let token_resp: TokenResponse = serde_json::from_str(&body)?;
            Some(token_resp.token)
        }
        RegistryAuth::GitLab {
            auth_url,
            service,
            client_id,
        } => {
            let token_url = format!(
                "{}?client_id={}&service={}&scope=repository:{}:pull",
                auth_url, client_id, service, image_name
            );

            let mut token_request = client.get(&token_url);
            if let Some((username, password)) = &creds {
                token_request = token_request.basic_auth(username, Some(password));
            }

            let response = token_request.send().await?;
            let body = response.text().await?;
            let token_resp: TokenResponse = serde_json::from_str(&body)?;
            Some(token_resp.token)
        }
        RegistryAuth::Generic { auth_url, service } => {
            let token_url = format!(
                "{}?service={}&scope=repository:{}:pull",
                auth_url, service, image_name
            );

            let mut token_request = client.get(&token_url);
            if let Some((username, password)) = creds {
                token_request = token_request.basic_auth(username, Some(password));
            }

            let response = token_request.send().await?;
            let body = response.text().await?;
            let token_resp: TokenResponse = serde_json::from_str(&body)?;
            Some(token_resp.token)
        }
    };

    info!("Received registry token for {}", registry);
    Ok(token)
}

/// Checks if a container image exists in a registry without pulling it
///
/// # Arguments
/// * `image_name` - Name of the image (e.g., "library/ubuntu")
/// * `tag` - Tag of the image (e.g., "latest")
/// * `registry` - Registry host (default: "registry.hub.docker.com")
///
/// # Returns
/// * `Result<bool>` - true if image exists, false otherwise
pub async fn image_exists(image_name: &str, tag: &str, registry: &str) -> Result<bool> {
    info!(
        "Checking if image exists: {}:{} in registry {}",
        image_name, tag, registry
    );
    let registry = registry;
    let client = Client::new();
    let creds = get_docker_credentials(registry);
    trace!("Creds for registry {} are {:#?}", registry, creds);

    // Get token if needed
    let token = get_registry_token(&client, registry, image_name, creds).await?;
    trace!("Token for registry {} is {:#?}", registry, token);

    // Check if the manifest exists
    let manifest_url = format!("https://{}/v2/{}/manifests/{}", registry, image_name, tag);

    // Pass token as Option<&str>
    check_manifest(&client, &manifest_url, token.as_deref()).await
}

async fn get_github_version(
    repo: &str,
    token: Option<String>,
    filter: String,
) -> Result<String, Box<dyn Error>> {
    info!("Getting latest version from Github for repo {}", repo);
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    debug!("Github query url {}", url);
    let mut request = client.get(url).header(USER_AGENT, USER_AGENT_NAME);
    trace!("Request for repo {}is {:?}", repo, request);

    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {}", token));
    }

    let response = request.send().await?;
    trace!("Response for repo {}is {:?}", repo, response);

    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        error!(
            "Rate limit exceeded for Github API when querying repo {}",
            repo
        );
        return Ok("<RATE_LIMITED>".to_string());
    }

    let body = response.text().await?;
    trace!("Body for repo {}is {:?}", repo, body);
    let data: serde_json::Value = serde_json::from_str(&body)?;
    trace!("Data for  repo {}is {:?}", repo, data);
    let tag_name = data["tag_name"].as_str().unwrap_or("");
    trace!("Tag for repo {}is {:?}", repo, tag_name);

    let version_pattern = format!("{}", filter); // Just use the filter directly
    let re = Regex::new(&version_pattern).unwrap();
    let version = re
        .captures(tag_name)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    if version.is_empty() {
        error!(
            "No version found matching filter '{}' in tag '{}' for repository '{}'",
            filter,
            tag_name,
            repo // or project_id for GitLab
        );
        return Err("No matching version found".into());
    }
    Ok(version)
}

async fn get_gitlab_version(
    repo: u64,
    token: Option<String>,
    filter: String,
) -> Result<String, Box<dyn Error>> {
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
        request = request.header("PRIVATE-TOKEN", format!("{}", token));
    }

    let response = request.send().await?;
    trace!("Response for repo {}is {:?}", repo, response);
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        error!(
            "Rate limit exceeded for Gitlab API when querying repo {}",
            repo
        );
        return Ok("<RATE_LIMITED>".to_string());
    }
    let body = response.text().await?;
    trace!("Body for repo {}is {:?}", repo, body);
    let data: serde_json::Value = serde_json::from_str(&body)?;
    trace!("Data for  repo {}is {:?}", repo, data);
    let tag_name = data["tag_name"].as_str().unwrap_or("");
    trace!("Tag for repo {}is {:?}", repo, tag_name);

    let version_pattern = format!("{}", filter); // Just use the filter directly
    let re = Regex::new(&version_pattern).unwrap();
    let version = re
        .captures(tag_name)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    if version.is_empty() {
        error!(
            "No version found matching filter '{}' in tag '{}' for repository '{}'",
            filter,
            tag_name,
            repo // or project_id for GitLab
        );
        return Err("No matching version found".into());
    }
    Ok(version)
}

fn extract_registry(full_image_name: &str) -> ImageParts {
    info!("Extracting image registry for image {}", full_image_name);
    // Matches FQDN pattern: contains dots, optional port number
    let re = Regex::new(r"^([a-zA-Z0-9][-a-zA-Z0-9.]*\.[a-zA-Z]{2,})(?::\d+)?/(.+)").unwrap();

    if let Some(captures) = re.captures(full_image_name) {
        // First capture group is registry, second is the rest of the path
        let registry = Some(captures.get(1).map(|m| m.as_str().to_string()).unwrap()).unwrap();
        let mut image_path = captures.get(2).map(|m| m.as_str().to_string()).unwrap();
        // Add library/ prefix if image_path doesn't contain a slash
        if !image_path.contains('/') {
            image_path = format!("library/{}", image_path);
        }
        info!("Found image {} with registry {}", image_path, &registry);
        ImageParts {
            registry,
            image_path,
        }
    } else {
        // No registry found, use default registry and check if path needs library/ prefix
        let image_path = if !full_image_name.contains('/') {
            debug!(
                "Found official docker image {}, prepending path with library/ ",
                full_image_name
            );
            format!("library/{}", full_image_name)
        } else {
            full_image_name.to_string()
        };

        ImageParts {
            registry: "registry.hub.docker.com".to_owned(),
            image_path,
        }
    }
}
#[derive(Debug)]
struct ImageParts {
    registry: String,
    image_path: String,
}

fn init_loggin() -> () {
    Builder::new()
        .format(|buf, record| {
            writeln!(
                buf,
                "[{}][{}] - {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .filter_level(log::LevelFilter::Info)
        .parse_env("RUST_LOG")
        .init();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_loggin();

    info!("Starting application");

    let args = Args::parse();
    info!("Reading config file: {}", args.config);
    let config_content = fs::read_to_string(&args.config)?;
    debug!("Config content read");
    let config_file: Config = serde_yaml::from_str(&config_content)?;
    trace!("Config content is {}", config_content);

    for (service_name, service_config) in &config_file.services {
        if let Err(e) = service_config.git.validate() {
            error!(
                "Invalid configuration for service '{}': {}",
                service_name, e
            );
            std::process::exit(1);
        };
    }

    let mut output = OutputData {
        versions: HashMap::new(),
    };

    // Process each service in the config
    for (service_name, service_config) in config_file.services {
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
                    service_config.git.filter,
                )
                .await?
            }
            GitType::None => String::new(),
        };
        info!("Found version {} for service {}", version, service_name);

        // Replace ${RELEASE_VERSION} placeholder if present
        let mut image_tag = service_config
            .image
            .tag
            .replace("${RELEASE_VERSION}", &version)
            .to_string();

        let image_parts = extract_registry(&service_config.image.name);

        let image_exists =
            match image_exists(&image_parts.image_path, &image_tag, &image_parts.registry).await {
                Ok(exists) => exists,
                Err(e) => {
                    error!("{}", e);
                    // Exit the program with an error code
                    std::process::exit(1);
                }
            };

        if !image_exists {
            error!(
                "Error: Image {}:{} does not exist in the registry",
                &service_config.image.name, &image_tag
            );
            image_tag = "<NOT_FOUND>".to_string();
        };

        // Add to output
        output.versions.insert(
            service_name,
            ServiceInfo {
                container_image: service_config.image.name.to_string(),
                image_tag: image_tag.to_string(),
            },
        );
    }

    // Output results in requested format
    let output_content = match args.format {
        OutputFormat::Json => serde_json::to_string_pretty(&output)?,
        OutputFormat::Yaml => serde_yaml::to_string(&output)?,
    };

    // Write to file
    info!("Writing output to file: {}", args.output);
    fs::write(&args.output, output_content)?;
    info!("Output written successfully");
    Ok(())
}
