use std::env;

#[derive(Clone)]
pub struct Config {
  pub port: u16,
  pub base_url: String,
  pub origin_url: String,
  pub is_dev: bool,
}

impl Config {
  pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
    // Kolla om dev mode
    let is_dev = env::var("DEV_MODE")
        .unwrap_or_else(|_| "false".to_string())
        .parse()
        .unwrap_or(false);
    
    // Port: required i prod, default i dev
    let port = if is_dev {
        env::var("PORT")
            .unwrap_or_else(|_| "3000".to_string())
            .parse()?
    } else {
        env::var("PORT")
            .map_err(|_| "PORT is required in production")?
            .parse()?
    };
    
    // Base URL: required i prod, default i dev
    let base_url = if is_dev {
        env::var("BASE_URL")
            .unwrap_or_else(|_| "http://localhost:3000".to_string())
    } else {
        env::var("BASE_URL")
            .map_err(|_| "BASE_URL is required in production")?
    };
    
    // Origin URL: required i prod, default i dev
    let origin_url = if is_dev {
        env::var("ORIGIN_URL")
            .unwrap_or_else(|_| "https://example.com".to_string())
    } else {
        env::var("ORIGIN_URL")
            .map_err(|_| "ORIGIN_URL is required in production")?
    };
    
    Ok(Config {
        port,
        base_url,
        origin_url,
        is_dev,
    })
}
}