use crate::git::USER_AGENT_NAME;

use super::error::AppError;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use log::{debug, info, trace, warn};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug)]
pub struct ImageParts {
    pub registry: String,
    pub image_path: String,
}
#[derive(Deserialize)]
struct DockerAuth {
    auth: Option<String>,
    username: Option<String>,
    password: Option<String>,
}
enum RegistryAuth {
    Standard {
        auth_url: String,
        service: String,
        client_id: Option<String>, // Optional for GitLab
    },
}

impl RegistryAuth {
    fn from_registry(registry: &str) -> Self {
        match registry {
            "registry.hub.docker.com" => RegistryAuth::Standard {
                auth_url: "https://auth.docker.io/token".to_string(),
                service: "registry.docker.io".to_string(),
                client_id: None,
            },
            r if r.contains("gitlab") => RegistryAuth::Standard {
                auth_url: "https://gitlab.com/jwt/auth".to_string(),
                service: "container_registry".to_string(),
                client_id: Some("docker".to_string()),
            },
            r if r.contains("ghcr.io") => RegistryAuth::Standard {
                auth_url: "https://ghcr.io/token".to_string(),
                service: "ghcr.io".to_string(),
                client_id: None,
            },
            _ => RegistryAuth::Standard {
                auth_url: format!("https://{}/v2/token", registry),
                service: registry.to_string(),
                client_id: None,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: String,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageConfig {
    pub name: String,
    pub tag: String,
}

#[derive(Deserialize)]
struct DockerConfig {
    auths: std::collections::HashMap<String, DockerAuth>,
}
pub struct RegistryClient {
    client: Client,
    registry: String,
    image_path: String,
}

impl RegistryClient {
    pub fn new(full_image_name: &str) -> Self {
        let image_parts = extract_registry(full_image_name);
        Self {
            client: Client::new(),
            registry: image_parts.registry,
            image_path: image_parts.image_path,
        }
    }

    pub async fn validate_tag(&self, tag: &str) -> Result<bool, AppError> {
        info!("Validating tag '{}' for image '{}'", tag, self.image_path);

        let creds = get_docker_credentials(&self.registry)
            .map_err(|e| AppError::CredentialsError(e.to_string()))?;

        let token = get_registry_token(&self.client, &self.registry, &self.image_path, creds)
            .await
            .map_err(|e| AppError::AuthenticationError(e.to_string()))?;

        let manifest_url = format!(
            "https://{}/v2/{}/manifests/{}",
            self.registry, self.image_path, tag
        );

        check_manifest(&self.client, &manifest_url, token.as_deref()).await
    }
}

pub fn get_docker_credentials(registry: &str) -> Result<Option<(String, String)>, AppError> {
    info!("Getting docker credentials for {}", registry);
    let config_path = dirs::home_dir().ok_or_else(|| {
        AppError::CredentialsError("Could not determine home directory".to_string())
    })?;
    let config_path = config_path.join(".docker/config.json");

    trace!("Trying to read docker credentials from ~/.docker/config.json");
    let config_contents = fs::read_to_string(config_path)
        .map_err(|e| AppError::CredentialsError(format!("Failed to read docker config: {}", e)))?;

    let config: DockerConfig = serde_json::from_str(&config_contents)
        .map_err(|e| AppError::CredentialsError(format!("Failed to parse docker config: {}", e)))?;

    if let Some(auth) = config.auths.get(registry) {
        // Try to get credentials from base64-encoded auth string
        if let Some(auth_str) = &auth.auth {
            let decoded = STANDARD.decode(auth_str).map_err(|e| {
                AppError::CredentialsError(format!("Failed to decode auth string: {}", e))
            })?;
            let decoded = String::from_utf8(decoded).map_err(|e| {
                AppError::CredentialsError(format!("Invalid UTF-8 in auth string: {}", e))
            })?;
            if let Some((username, password)) = decoded.split_once(':') {
                return Ok(Some((username.to_string(), password.to_string())));
            }
        }

        // If no auth string, try explicit username/password
        if let (Some(username), Some(password)) = (&auth.username, &auth.password) {
            return Ok(Some((username.clone(), password.clone())));
        }
    }

    Ok(None)
}

pub async fn check_manifest(
    client: &Client,
    manifest_url: &str,
    token: Option<&str>,
) -> Result<bool, AppError> {
    info!("Getting image manifest at URL: {}", manifest_url);
    let accept_headers = [
        "application/vnd.docker.distribution.manifest.v2+json",
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
    ];

    for accept in accept_headers {
        debug!("Trying manifest format: {}", accept);

        let mut request = client
            .get(manifest_url)
            .header("Accept", accept)
            .header(USER_AGENT, USER_AGENT_NAME);

        // Only add authorization header if token is present
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = request.send().await.map_err(|e| {
            AppError::RequestError(format!("Failed to send manifest request: {}", e))
        })?;

        match response.status() {
            StatusCode::OK => {
                info!(
                    "Successfully found manifest at {} with accept header: {}",
                    manifest_url, accept
                );
                return Ok(true);
            }
            StatusCode::NOT_FOUND => {
                if let Ok(error_body) = response.text().await {
                    let is_last_header = accept == accept_headers[accept_headers.len() - 1];
                    warn!(
                        "Manifest not found with accept header: {}{}",
                        accept,
                        if !is_last_header {
                            ". Trying next accept header"
                        } else {
                            ""
                        }
                    );
                    debug!(
                        "Got 404 with for accept header {} with error body: {}",
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
                let error_body = response.text().await.map_err(|e| {
                    AppError::RequestError(format!("Failed to read response body: {}", e))
                })?;
                return Err(AppError::RateLimited(error_body));
            }
            status => {
                let error_body = response.text().await.unwrap_or_default();
                return Err(AppError::RequestError(format!(
                    "Unexpected status code: {} with body: {}",
                    status, error_body
                )));
            }
        }
    }

    Err(AppError::ImageNotFound(format!(
        "No manifest found for {}",
        manifest_url
    )))
}

pub async fn get_registry_token(
    client: &Client,
    registry: &str,
    image_name: &str,
    creds: Option<(String, String)>,
) -> Result<Option<String>, AppError> {
    if registry.contains("quay.io") {
        return Ok(None);
    }

    info!("Getting registry token for {}", registry);

    let auth = RegistryAuth::from_registry(registry);
    let token = get_token(client, &auth, image_name, creds).await?;

    Ok(Some(token))
}

async fn get_token(
    client: &Client,
    auth: &RegistryAuth,
    image_name: &str,
    creds: Option<(String, String)>,
) -> Result<String, AppError> {
    let RegistryAuth::Standard {
        auth_url,
        service,
        client_id,
    } = auth;

    let mut token_url = format!(
        "{}?service={}&scope=repository:{}:pull",
        auth_url, service, image_name
    );

    // Add client_id parameter for GitLab if present
    if let Some(client_id) = client_id {
        token_url.push_str(&format!("&client_id={}", client_id));
    }

    trace!("token url: {}", token_url);
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_NAME));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    let mut token_request = client.get(&token_url).headers(headers);

    // Handle authentication
    if service == "ghcr.io" {
        // Try GITHUB_TOKEN first
        if let Ok(github_token) = std::env::var("GITHUB_TOKEN") {
            token_request = token_request.header(
                "Authorization",
                HeaderValue::from_str(&format!("Bearer {}", github_token))
                    .map_err(|e| AppError::AuthenticationError(e.to_string()))?,
            );
        } else if let Some((username, password)) = creds {
            // Fall back to basic auth only if GITHUB_TOKEN is not present
            token_request = token_request.basic_auth(username, Some(password));
        }
    } else if let Some((username, password)) = creds {
        // For non-ghcr.io services, use basic auth if credentials are available
        token_request = token_request.basic_auth(username, Some(password));
    }
    trace!("token request client is: {:?}", token_request);

    let response = token_request.send().await.map_err(|e| {
        AppError::AuthenticationError(format!("Failed to send token request: {}", e))
    })?;

    let body = response.text().await.map_err(|e| {
        AppError::AuthenticationError(format!("Failed to read token response: {}", e))
    })?;

    trace!(
        "token response for service {} with token_url {}: {}",
        service,
        token_url,
        body
    );
    let token_resp: TokenResponse = serde_json::from_str(&body)
        .map_err(|e| AppError::InvalidResponse(format!("Failed to parse token response: {}", e)))?;

    Ok(token_resp.token)
}

// Helper function
fn extract_registry(full_image_name: &str) -> ImageParts {
    info!("Extracting image registry for image {}", full_image_name);
    // Matches FQDN pattern: contains dots, optional port number
    let re = Regex::new(r"^([a-zA-Z0-9][-a-zA-Z0-9.]*\.[a-zA-Z]{2,})(?::\d+)?/(.+)").unwrap();

    if let Some(captures) = re.captures(full_image_name) {
        // First capture group is registry, second is the rest of the path
        let registry = captures.get(1).map(|m| m.as_str().to_string()).unwrap();
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
