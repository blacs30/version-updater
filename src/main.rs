mod config;
mod error;
mod git;
mod logging;
mod registry;
mod service;

use anyhow::Result;
use config::{AppConfig, Args, OutputData, OutputFormat, ServiceVersion};
use futures::future::join_all;
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

    // Create a vector of futures for all service processing tasks
    let processing_tasks: Vec<_> = config
        .services
        .iter()
        .map(|(name, service_config)| {
            let name = name.clone();
            let processor = ServiceProcessor::new(service_config.clone());
            async move {
                let result = processor.process().await;
                (name, result)
            }
        })
        .collect();

    // Execute all tasks concurrently
    let results = join_all(processing_tasks).await;

    // Process results
    for (name, result) in results {
        match result {
            Ok(service_info) => {
                output.insert(name, service_info);
            }
            Err(e) => {
                error!("Failed to process service '{}': {}", name, e);
                output.insert(
                    name.clone(),
                    ServiceVersion::error(
                        config.services[&name].image.name.clone(),
                        &format!("Processing failed: {}", e),
                    ),
                );
            }
        }
    }

    Ok(output)
}
