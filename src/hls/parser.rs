use crate::error::{Result, RitcherError};
use m3u8_rs::{parse_playlist_res, Playlist};
use tracing::info;

/// Parse HLS playlist from string content
pub fn parse_hls_playlist(content: &str) -> Result<Playlist> {
    info!("Parsing HLS playlist");

    match parse_playlist_res(content.as_bytes()) {
        Ok(playlist) => {
            info!("Successfully parsed playlist");
            Ok(playlist)
        }
        Err(e) => {
            let error_msg = format!("Failed to parse playlist: {:?}", e);
            Err(RitcherError::PlaylistParseError(error_msg))
        }
    }
}

/// Modify playlist by rewriting segment URLs to route through stitcher
pub fn modify_playlist(
    mut playlist: Playlist,
    session_id: &str,
    base_url: &str,
    origin_url: &str,
) -> Result<String> {
    info!("Modifying playlist for session: {}", session_id);

    if let Playlist::MediaPlaylist(ref mut media_playlist) = playlist {
        for (index, segment) in media_playlist.segments.iter_mut().enumerate() {
            info!("Original segment URL: {}", segment.uri);

            // AD INSERTION LOGIC: Every 10th segment becomes an ad
            // TODO: This should be replaced with proper SCTE-35 marker detection
            if index > 0 && index % 10 == 0 {
                info!("🎬 INSERTING AD at segment #{}", index);

                segment.discontinuity = true;

                segment.uri = format!("{}/stitch/{}/ad/ad-segment.ts", base_url, session_id);
            } else {
                // Normal content segment - rewrite URL to proxy through stitcher
                let segment_name = if segment.uri.starts_with("http") {
                    segment.uri.split('/').next_back().unwrap_or(&segment.uri)
                } else {
                    &segment.uri
                };

                segment.uri = format!(
                    "{}/stitch/{}/segment/{}?origin={}",
                    base_url, session_id, segment_name, origin_url
                );
            }
        }
    }

    // Serialize modified playlist back to string
    let mut output = Vec::new();
    playlist
        .write_to(&mut output)
        .map_err(|e| RitcherError::PlaylistModifyError(format!("Failed to write playlist: {}", e)))?;

    String::from_utf8(output).map_err(|e| {
        RitcherError::ConversionError(format!("Failed to convert playlist to UTF-8: {}", e))
    })
}
