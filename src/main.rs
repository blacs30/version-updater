mod config;
mod error;
mod git;
mod logging;
mod registry;
mod service;

use anyhow::Result;
use config::{AppConfig, Args, OutputData, OutputFormat, ServiceConfig, ServiceInfo};
use log::{error, info, warn};
use logging::init_logging;
use service::ServiceProcessor;
use std::fs;

// main.rs
#[tokio::main]
async fn main() -> Result<()> {
    init_logging(Some(log::LevelFilter::Info));
    let config = AppConfig::load_config()?;
    let output = process_services(&config).await?;
    write_output(&output, &config.args)?;

    // Optionally, you could check if any services failed
    let failed_services: Vec<_> = output
        .versions
        .iter()
        .filter(|(_, info)| info.error.is_some())
        .collect();

    if !failed_services.is_empty() {
        warn!("{} services failed to process:", failed_services.len());
        for (name, info) in failed_services {
            warn!("  {}: {}", name, info.error.as_ref().unwrap());
        }
    }

    Ok(())
}

fn write_output(output: &OutputData, args: &Args) -> Result<()> {
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

async fn process_services(config: &AppConfig) -> Result<OutputData> {
    let mut output = OutputData::new();

    for (name, service_config) in &config.services {
        match process_single_service(service_config).await {
            Ok(service_info) => {
                output.add_service(name.clone(), service_info);
            }
            Err(e) => {
                // Log the error but continue processing other services
                error!("Failed to process service '{}': {}", name, e);
                // Add an error entry for this service
                output.add_service(
                    name.clone(),
                    ServiceInfo {
                        container_image: service_config.image.name.clone(),
                        image_tag: format!("<ERROR: {}>", e),
                        error: None,
                    },
                );
            }
        }
    }

    Ok(output)
}

async fn process_single_service(service_config: &ServiceConfig) -> Result<ServiceInfo> {
    let processor = ServiceProcessor::new(service_config.clone());
    processor.process().await
}
