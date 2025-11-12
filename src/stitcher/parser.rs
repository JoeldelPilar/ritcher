use m3u8_rs::{parse_playlist_res, Playlist};
use tracing::{info, error};

pub fn parse_hls_playlist(content: &str) -> Result<Playlist, Box<dyn std::error::Error>> {
    info!("Parsing HLS playlist");
    
    match parse_playlist_res(content.as_bytes()) {
        Ok(playlist) => {
            info!("Successfully parsed playlist");
            Ok(playlist)
        }
        Err(e) => {
            error!("Failed to parse playlist: {:?}", e);
            Err("Failed to parse playlist".into())
        }
    }
}

pub fn modify_playlist(playlist: Playlist, session_id: &str) -> Result<String, Box<dyn std::error::Error>> {
  info!("Modifying playlist for session: {}", session_id);

  // TODO: Modify segment URLs here

  // For now, just return the playlist as a string
  let mut output = Vec::new();
  playlist.write_to(&mut output)?;
  Ok(String::from_utf8(output)?)
}