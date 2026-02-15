use tracing::{error, info};

mod config;
mod error;
mod hls;
mod server;
mod session;

#[tokio::main]
async fn main() {
    // Setup logging
    tracing_subscriber::fmt::init();

    info!("🚀 Starting Ritcher - Rust HLS Stitcher");

    let config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    info!(
        "Running in {} mode",
        if config.is_dev { "DEV" } else { "PROD" }
    );

    if let Err(e) = server::start(config).await {
        error!("Failed to start server: {}", e);
        std::process::exit(1);
    }
}