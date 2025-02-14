use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use log::{debug, error, info, trace, warn};
use regex::Regex;
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
    DockerHub {
        auth_url: String,
        service: String,
    },
    GitLab {
        auth_url: String,
        service: String,
        client_id: String,
    },
    GitHub {
        auth_url: String,
        service: String,
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
            r if r.contains("ghcr.io") => RegistryAuth::GitHub {
                auth_url: "https://ghcr.io/token".to_string(),
                service: "ghcr.io".to_string(),
            },
            _ => RegistryAuth::Generic {
                auth_url: format!("https://{}/v2/token", registry),
                service: registry.to_string(),
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

    pub async fn validate_tag(&self, tag: &str) -> Result<bool> {
        info!("Validating tag '{}' for image '{}'", tag, self.image_path);

        let creds = get_docker_credentials(&self.registry);
        let token =
            get_registry_token(&self.client, &self.registry, &self.image_path, creds).await?;

        let manifest_url = format!(
            "https://{}/v2/{}/manifests/{}",
            self.registry, self.image_path, tag
        );

        check_manifest(&self.client, &manifest_url, token.as_deref()).await
    }
}

pub fn get_docker_credentials(registry: &str) -> Option<(String, String)> {
    info!("Gettinig docker credentials for {}", registry);
    let config_path = dirs::home_dir()?.join(".docker/config.json");
    trace!("Trying to read docker credentials from ~/.docker/config.json");
    let config_contents = fs::read_to_string(config_path).ok()?;
    let config: DockerConfig = serde_json::from_str(&config_contents).ok()?;

    let auth = config.auths.get(registry)?;

    // If we have a base64-encoded auth string, decode it
    if let Some(auth_str) = &auth.auth {
        info!("Found docker credentials base64 encoded for {}", registry);
        let decoded = String::from_utf8(STANDARD.decode(auth_str).ok()?).ok()?;
        let (username, password) = decoded.split_once(':')?;
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

pub async fn check_manifest(
    client: &Client,
    manifest_url: &str,
    token: Option<&str>,
) -> Result<bool> {
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
                let error_body = response.text().await?;
                error!(
                    "Rate limit hit for: {}. Error: {}",
                    manifest_url, error_body
                );
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

pub async fn get_registry_token(
    client: &Client,
    registry: &str,
    image_name: &str,
    creds: Option<(String, String)>,
) -> Result<Option<String>> {
    // Skip authentication for quay.io
    if registry.contains("quay.io") {
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
        RegistryAuth::Generic { auth_url, service }
        | RegistryAuth::GitHub { auth_url, service } => {
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
