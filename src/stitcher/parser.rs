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

pub fn modify_playlist(mut playlist: Playlist, session_id: &str, base_url: &str) -> Result<String, Box<dyn std::error::Error>> {
  info!("Modifying playlist for session: {}", session_id);

  if let Playlist::MediaPlaylist(ref mut media_playlist) = playlist {
    for segment in &mut media_playlist.segments {
      info!("original segment URL: {}", segment.uri);

      let segment_name = segment.uri.split('/').last().unwrap_or(&segment.uri);

      info!("segment name: {}", segment_name);

      let new_uri = format!("{}/stitch/{}/segment/{}",
        base_url,
        session_id,
        segment_name
      );

      info!("new segment URL: {}", new_uri);

      segment.uri = new_uri;
    }
  }

  // For now, just return the playlist as a string
  let mut output = Vec::new();
  playlist.write_to(&mut output)?;
  Ok(String::from_utf8(output)?)
}