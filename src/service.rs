use super::config::{ServiceConfig, ServiceInfo};
use super::git::GitClient;
use super::registry::RegistryClient;
use log::error;

use anyhow::Result;

pub struct ServiceProcessor {
    config: ServiceConfig,
}

impl ServiceProcessor {
    pub fn new(config: ServiceConfig) -> Self {
        Self { config }
    }

    pub async fn process(&self) -> Result<ServiceInfo> {
        let version = match self.get_version().await {
            Ok(v) => v,
            Err(e) => {
                return Ok(ServiceInfo::error(
                    self.config.image.name.clone(),
                    &format!("Failed to get version: {}", e),
                ));
            }
        };

        let image_tag = match self.validate_image_tag(&version).await {
            Ok(tag) => tag,
            Err(e) => {
                return Ok(ServiceInfo::error(
                    self.config.image.name.clone(),
                    &format!("Failed to validate image tag: {}", e),
                ));
            }
        };

        Ok(ServiceInfo {
            container_image: self.config.image.name.clone(),
            image_tag,
            error: None,
        })
    }

    async fn get_version(&self) -> Result<String> {
        GitClient::get_version(&self.config.git).await
    }

    async fn validate_image_tag(&self, version: &str) -> Result<String> {
        let image_tag = self.config.image.tag.replace("${RELEASE_VERSION}", version);

        let registry_client = RegistryClient::new(&self.config.image.name);

        let exists = registry_client.validate_tag(&image_tag).await?;

        if !exists {
            error!(
                "Image {}:{} does not exist in the registry",
                self.config.image.name, image_tag
            );

            return Ok(match version {
                "<RATE_LIMITED>" => "<RATE_LIMITED>".to_string(),
                _ => "<NOT_FOUND>".to_string(),
            });
        }

        Ok(image_tag)
    }
}
