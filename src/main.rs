use tracing::{info, error};
use tracing_subscriber;

mod config;
mod server;
mod stitcher;
mod models;

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

    info!("Running in {} mode", if config.is_dev { "DEV" } else { "PROD" });
    
    if let Err(e) = server::start(config).await {
      error!("Failed to start server: {}", e);
      std::process::exit(1);
    }
}