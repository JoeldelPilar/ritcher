use crate::error::{Result, RitcherError};
use m3u8_rs::{AlternativeMediaType, Playlist, parse_playlist_res};
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

/// Rewrite content segment URLs to route through stitcher's proxy
///
/// This function ONLY handles URL rewriting for content segments.
/// Ad insertion is handled separately by the ad interleaver.
///
/// For segments with absolute URLs (starting with http), the origin is
/// derived from the segment's own URL. For relative URLs, the provided
/// origin_base is used as the origin.
///
/// Note: URLs with query parameters are currently not handled specially.
/// The query string will be included in the segment name passed to the
/// segment handler.
pub fn rewrite_content_urls(
    mut playlist: Playlist,
    session_id: &str,
    base_url: &str,
    origin_base: &str,
) -> Result<Playlist> {
    info!("Rewriting content URLs for session: {}", session_id);

    if let Playlist::MediaPlaylist(ref mut media_playlist) = playlist {
        for segment in media_playlist.segments.iter_mut() {
            // Skip segments that are already routed through stitcher (ads)
            if segment.uri.contains("/stitch/") {
                continue;
            }

            info!("Rewriting segment URL: {}", segment.uri);

            if segment.uri.starts_with("http") {
                // Absolute URL: derive origin from the segment's own URL
                let (seg_origin, segment_name) =
                    segment.uri.rsplit_once('/').unwrap_or(("", &segment.uri));

                segment.uri = format!(
                    "{}/stitch/{}/segment/{}?origin={}",
                    base_url, session_id, segment_name, seg_origin
                );
            } else {
                // Relative URL: use the provided origin base
                segment.uri = format!(
                    "{}/stitch/{}/segment/{}?origin={}",
                    base_url, session_id, segment.uri, origin_base
                );
            }
        }
    }

    Ok(playlist)
}

/// Rewrite master playlist variant-stream URLs to route through stitcher
///
/// Each variant stream's URI is rewritten to point to the stitcher's
/// playlist endpoint, with the original variant URL passed as the `origin`
/// query parameter. This ensures all quality levels are stitched.
///
/// Example transformation:
/// - Input:  `720p/playlist.m3u8`
/// - Output: `{base_url}/stitch/{session_id}/playlist.m3u8?origin={origin_base}/720p/playlist.m3u8`
pub fn rewrite_master_urls(
    mut playlist: Playlist,
    session_id: &str,
    base_url: &str,
    origin_base: &str,
) -> Result<Playlist> {
    info!("Rewriting master playlist URLs for session: {}", session_id);

    if let Playlist::MasterPlaylist(ref mut master) = playlist {
        for variant in master.variants.iter_mut() {
            let original_uri = variant.uri.clone();

            // Resolve the variant URI to an absolute URL
            let absolute_url = if variant.uri.starts_with("http") {
                variant.uri.clone()
            } else {
                format!("{}/{}", origin_base, variant.uri)
            };

            // Rewrite to route through stitcher
            variant.uri = format!(
                "{}/stitch/{}/playlist.m3u8?origin={}",
                base_url, session_id, absolute_url
            );

            info!("Rewrote variant: {} → {}", original_uri, variant.uri);
        }

        // Also rewrite alternative media URIs (audio, subtitle renditions)
        // Append &track= parameter so the playlist handler knows the track type
        for alt in master.alternatives.iter_mut() {
            if let Some(ref mut uri) = alt.uri {
                let original_uri = uri.clone();

                let absolute_url = if uri.starts_with("http") {
                    uri.clone()
                } else {
                    format!("{}/{}", origin_base, uri)
                };

                let track_type = match alt.media_type {
                    AlternativeMediaType::Audio => "audio",
                    AlternativeMediaType::Subtitles | AlternativeMediaType::ClosedCaptions => {
                        "subtitles"
                    }
                    _ => "video",
                };

                *uri = format!(
                    "{}/stitch/{}/playlist.m3u8?origin={}&track={}",
                    base_url, session_id, absolute_url, track_type
                );

                info!(
                    "Rewrote alternative media ({}): {} → {}",
                    track_type, original_uri, uri
                );
            }
        }

        info!(
            "Rewrote {} variant(s) and {} alternative(s) in master playlist",
            master.variants.len(),
            master.alternatives.len()
        );
    }

    Ok(playlist)
}

/// Serialize playlist to string
pub fn serialize_playlist(playlist: Playlist) -> Result<String> {
    let mut output = Vec::new();
    playlist.write_to(&mut output).map_err(|e| {
        RitcherError::PlaylistModifyError(format!("Failed to write playlist: {}", e))
    })?;

    String::from_utf8(output).map_err(|e| {
        RitcherError::ConversionError(format!("Failed to convert playlist to UTF-8: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3u8_rs::{AlternativeMedia, MasterPlaylist, VariantStream};

    #[test]
    fn test_rewrite_master_urls_relative() {
        let playlist = Playlist::MasterPlaylist(MasterPlaylist {
            variants: vec![
                VariantStream {
                    uri: "720p/playlist.m3u8".to_string(),
                    bandwidth: 2_000_000,
                    ..Default::default()
                },
                VariantStream {
                    uri: "1080p/playlist.m3u8".to_string(),
                    bandwidth: 5_000_000,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let result = rewrite_master_urls(
            playlist,
            "session-1",
            "http://stitcher.example.com",
            "http://cdn.example.com/stream",
        )
        .unwrap();

        if let Playlist::MasterPlaylist(master) = result {
            assert_eq!(master.variants.len(), 2);
            assert_eq!(
                master.variants[0].uri,
                "http://stitcher.example.com/stitch/session-1/playlist.m3u8?origin=http://cdn.example.com/stream/720p/playlist.m3u8"
            );
            assert_eq!(
                master.variants[1].uri,
                "http://stitcher.example.com/stitch/session-1/playlist.m3u8?origin=http://cdn.example.com/stream/1080p/playlist.m3u8"
            );
        } else {
            panic!("Expected MasterPlaylist");
        }
    }

    #[test]
    fn test_rewrite_master_urls_absolute() {
        let playlist = Playlist::MasterPlaylist(MasterPlaylist {
            variants: vec![VariantStream {
                uri: "http://other-cdn.example.com/720p/playlist.m3u8".to_string(),
                bandwidth: 2_000_000,
                ..Default::default()
            }],
            ..Default::default()
        });

        let result = rewrite_master_urls(
            playlist,
            "session-1",
            "http://stitcher.example.com",
            "http://cdn.example.com/stream",
        )
        .unwrap();

        if let Playlist::MasterPlaylist(master) = result {
            assert_eq!(
                master.variants[0].uri,
                "http://stitcher.example.com/stitch/session-1/playlist.m3u8?origin=http://other-cdn.example.com/720p/playlist.m3u8"
            );
        } else {
            panic!("Expected MasterPlaylist");
        }
    }

    #[test]
    fn test_rewrite_master_urls_with_alternatives() {
        let playlist = Playlist::MasterPlaylist(MasterPlaylist {
            variants: vec![VariantStream {
                uri: "video/playlist.m3u8".to_string(),
                bandwidth: 2_000_000,
                ..Default::default()
            }],
            alternatives: vec![AlternativeMedia {
                media_type: AlternativeMediaType::Audio,
                uri: Some("audio/en/playlist.m3u8".to_string()),
                group_id: "audio".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });

        let result = rewrite_master_urls(
            playlist,
            "session-1",
            "http://stitcher.example.com",
            "http://cdn.example.com/stream",
        )
        .unwrap();

        if let Playlist::MasterPlaylist(master) = result {
            assert_eq!(
                master.alternatives[0].uri.as_deref().unwrap(),
                "http://stitcher.example.com/stitch/session-1/playlist.m3u8?origin=http://cdn.example.com/stream/audio/en/playlist.m3u8&track=audio"
            );
        } else {
            panic!("Expected MasterPlaylist");
        }
    }

    #[test]
    fn test_rewrite_master_urls_adds_track_param_for_subtitles() {
        let playlist = Playlist::MasterPlaylist(MasterPlaylist {
            variants: vec![VariantStream {
                uri: "video/playlist.m3u8".to_string(),
                bandwidth: 2_000_000,
                ..Default::default()
            }],
            alternatives: vec![AlternativeMedia {
                media_type: AlternativeMediaType::Subtitles,
                uri: Some("subs/en/playlist.m3u8".to_string()),
                group_id: "subs".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });

        let result = rewrite_master_urls(
            playlist,
            "session-1",
            "http://stitcher.example.com",
            "http://cdn.example.com/stream",
        )
        .unwrap();

        if let Playlist::MasterPlaylist(master) = result {
            let uri = master.alternatives[0].uri.as_deref().unwrap();
            assert!(
                uri.ends_with("&track=subtitles"),
                "Expected track=subtitles, got: {}",
                uri
            );
        } else {
            panic!("Expected MasterPlaylist");
        }
    }

    #[test]
    fn test_rewrite_master_urls_no_track_param_for_video_variants() {
        let playlist = Playlist::MasterPlaylist(MasterPlaylist {
            variants: vec![VariantStream {
                uri: "720p/playlist.m3u8".to_string(),
                bandwidth: 2_000_000,
                ..Default::default()
            }],
            ..Default::default()
        });

        let result = rewrite_master_urls(
            playlist,
            "session-1",
            "http://stitcher.example.com",
            "http://cdn.example.com/stream",
        )
        .unwrap();

        if let Playlist::MasterPlaylist(master) = result {
            let uri = &master.variants[0].uri;
            assert!(
                !uri.contains("&track="),
                "Video variants should not have track param: {}",
                uri
            );
        } else {
            panic!("Expected MasterPlaylist");
        }
    }

    #[test]
    fn test_parse_and_serialize_roundtrip() {
        let m3u8_content = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nseg0.ts\n#EXTINF:10,\nseg1.ts\n#EXT-X-ENDLIST\n";

        let playlist = parse_hls_playlist(m3u8_content).unwrap();
        let serialized = serialize_playlist(playlist).unwrap();

        assert!(serialized.contains("#EXTM3U"));
        assert!(serialized.contains("seg0.ts"));
        assert!(serialized.contains("seg1.ts"));
    }
}
