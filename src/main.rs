use tracing::info;
use tracing_subscriber;

mod config;
mod server;
mod stitcher;
mod models;

#[tokio::main]
async fn main() {
    // Setup logging
    tracing_subscriber::fmt::init();
    
    info!("🦀 Starting Ritcher - Rust HLS Stitcher");
    
    // Start HTTP server
    server::start().await;
}