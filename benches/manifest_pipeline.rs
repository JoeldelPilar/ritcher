//! Benchmarks for the manifest stitching pipeline
//!
//! Tests the hot path: parse → detect CUE breaks → interleave ads → rewrite URLs → serialize
//!
//! This is the critical path executed for every manifest request in live SSAI.
//! In production, each concurrent viewer triggers this pipeline every segment
//! duration (~6 seconds), meaning 10,000 viewers = ~1,667 pipeline executions/sec.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use m3u8_rs::Playlist;
use ritcher::ad::interleaver;
use ritcher::ad::provider::AdSegment;
use ritcher::hls::cue;
use ritcher::hls::parser;

/// Generate a realistic live HLS media playlist with CUE markers
///
/// Simulates a live sliding window with configurable segment count and ad breaks.
/// Ad breaks use real SCTE-35 CUE-OUT/CUE-OUT-CONT/CUE-IN tag patterns.
fn generate_playlist(
    segment_count: usize,
    ad_break_count: usize,
    ad_break_duration: f32,
) -> String {
    let mut lines = vec![
        "#EXTM3U".to_string(),
        "#EXT-X-VERSION:3".to_string(),
        format!("#EXT-X-TARGETDURATION:6"),
        format!("#EXT-X-MEDIA-SEQUENCE:1000"),
    ];

    let segments_per_ad_break = (ad_break_duration / 6.0).ceil() as usize;
    let total_ad_segments: usize = ad_break_count * segments_per_ad_break;
    let content_segments = segment_count.saturating_sub(total_ad_segments);
    let content_between_breaks = if ad_break_count > 0 {
        content_segments / (ad_break_count + 1)
    } else {
        content_segments
    };

    let mut seg_num = 0;
    for break_idx in 0..=ad_break_count {
        // Content segments
        let count = if break_idx < ad_break_count {
            content_between_breaks
        } else {
            content_segments - (content_between_breaks * ad_break_count)
        };

        for _ in 0..count {
            lines.push("#EXTINF:6.006,".to_string());
            lines.push(format!(
                "https://cdn.example.com/stream/segment_{}.ts",
                seg_num
            ));
            seg_num += 1;
        }

        // Ad break (except after last content block)
        if break_idx < ad_break_count {
            // CUE-OUT
            lines.push(format!("#EXT-X-CUE-OUT:{}", ad_break_duration));
            lines.push("#EXTINF:6.006,".to_string());
            lines.push(format!(
                "https://cdn.example.com/stream/segment_{}.ts",
                seg_num
            ));
            seg_num += 1;

            // CUE-OUT-CONT for middle segments
            for cont_idx in 1..segments_per_ad_break.saturating_sub(1) {
                let elapsed = (cont_idx as f32 + 1.0) * 6.0;
                lines.push(format!(
                    "#EXT-X-CUE-OUT-CONT:{}/{}",
                    elapsed, ad_break_duration
                ));
                lines.push("#EXTINF:6.006,".to_string());
                lines.push(format!(
                    "https://cdn.example.com/stream/segment_{}.ts",
                    seg_num
                ));
                seg_num += 1;
            }

            // CUE-IN
            lines.push("#EXT-X-CUE-IN".to_string());
            lines.push("#EXTINF:6.006,".to_string());
            lines.push(format!(
                "https://cdn.example.com/stream/segment_{}.ts",
                seg_num
            ));
            seg_num += 1;
        }
    }

    lines.join("\n") + "\n"
}

/// Generate a master playlist with multiple variants
fn generate_master_playlist(variant_count: usize) -> String {
    let mut lines = vec!["#EXTM3U".to_string()];

    let resolutions = [
        ("426x240", 400_000),
        ("640x360", 800_000),
        ("854x480", 1_400_000),
        ("1280x720", 2_800_000),
        ("1920x1080", 5_000_000),
        ("2560x1440", 8_000_000),
        ("3840x2160", 14_000_000),
    ];

    for i in 0..variant_count {
        let (res, bw) = resolutions[i % resolutions.len()];
        lines.push(format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}",
            bw, res
        ));
        lines.push(format!("variant_{}/playlist.m3u8", i));
    }

    lines.join("\n") + "\n"
}

/// Generate mock ad segments for a given duration
fn generate_ad_segments(duration: f32, segment_duration: f32) -> Vec<AdSegment> {
    let count = (duration / segment_duration).ceil() as usize;
    (0..count)
        .map(|i| AdSegment {
            uri: format!("ad-segment-{}.ts", i),
            duration: segment_duration,
            tracking: None,
        })
        .collect()
}

// ── Benchmarks ──────────────────────────────────────────────────────

/// Benchmark: Parse HLS media playlist
fn bench_parse_playlist(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_playlist");

    for segment_count in [6, 12, 30, 60] {
        let playlist_str = generate_playlist(segment_count, 1, 30.0);

        group.bench_with_input(
            BenchmarkId::new("segments", segment_count),
            &playlist_str,
            |b, input| {
                b.iter(|| {
                    parser::parse_hls_playlist(black_box(input)).unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Detect CUE ad breaks
fn bench_detect_cue_breaks(c: &mut Criterion) {
    let mut group = c.benchmark_group("detect_cue_breaks");

    for (ad_breaks, label) in [(1, "1_break"), (3, "3_breaks"), (5, "5_breaks")] {
        let playlist_str = generate_playlist(30, ad_breaks, 30.0);
        let parsed = parser::parse_hls_playlist(&playlist_str).unwrap();
        let media = match parsed {
            Playlist::MediaPlaylist(mp) => mp,
            _ => panic!("Expected MediaPlaylist"),
        };

        group.bench_with_input(BenchmarkId::new("ad_breaks", label), &media, |b, input| {
            b.iter(|| {
                cue::detect_ad_breaks(black_box(input));
            });
        });
    }

    group.finish();
}

/// Benchmark: Interleave ad segments into playlist
fn bench_interleave_ads(c: &mut Criterion) {
    let mut group = c.benchmark_group("interleave_ads");

    for (ad_breaks, label) in [(1, "1_break"), (3, "3_breaks")] {
        let playlist_str = generate_playlist(30, ad_breaks, 30.0);
        let parsed = parser::parse_hls_playlist(&playlist_str).unwrap();
        let media = match parsed {
            Playlist::MediaPlaylist(mp) => mp,
            _ => panic!("Expected MediaPlaylist"),
        };

        let breaks = cue::detect_ad_breaks(&media);
        let ad_segments: Vec<Vec<AdSegment>> = breaks
            .iter()
            .map(|ab| generate_ad_segments(ab.duration, 6.0))
            .collect();

        group.bench_with_input(
            BenchmarkId::new("ad_breaks", label),
            &(media.clone(), breaks.clone(), ad_segments.clone()),
            |b, (media, breaks, ads)| {
                b.iter(|| {
                    interleaver::interleave_ads(
                        black_box(media.clone()),
                        black_box(breaks),
                        black_box(ads),
                        "bench-session",
                        "http://stitcher.example.com",
                    );
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Rewrite content URLs
fn bench_rewrite_urls(c: &mut Criterion) {
    let mut group = c.benchmark_group("rewrite_urls");

    for segment_count in [6, 30, 60] {
        let playlist_str = generate_playlist(segment_count, 0, 0.0);
        let parsed = parser::parse_hls_playlist(&playlist_str).unwrap();

        group.bench_with_input(
            BenchmarkId::new("segments", segment_count),
            &parsed,
            |b, input| {
                b.iter(|| {
                    parser::rewrite_content_urls(
                        black_box(input.clone()),
                        "bench-session",
                        "http://stitcher.example.com",
                        "https://cdn.example.com/stream",
                    )
                    .unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Rewrite master playlist URLs
fn bench_rewrite_master(c: &mut Criterion) {
    let mut group = c.benchmark_group("rewrite_master");

    for variant_count in [3, 5, 7] {
        let playlist_str = generate_master_playlist(variant_count);
        let parsed = parser::parse_hls_playlist(&playlist_str).unwrap();

        group.bench_with_input(
            BenchmarkId::new("variants", variant_count),
            &parsed,
            |b, input| {
                b.iter(|| {
                    parser::rewrite_master_urls(
                        black_box(input.clone()),
                        "bench-session",
                        "http://stitcher.example.com",
                        "https://cdn.example.com/stream",
                    )
                    .unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Serialize playlist to string
fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize_playlist");

    for segment_count in [6, 30, 60] {
        let playlist_str = generate_playlist(segment_count, 1, 30.0);
        let parsed = parser::parse_hls_playlist(&playlist_str).unwrap();

        group.bench_with_input(
            BenchmarkId::new("segments", segment_count),
            &parsed,
            |b, input| {
                b.iter(|| {
                    parser::serialize_playlist(black_box(input.clone())).unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Full pipeline (parse → detect → interleave → rewrite → serialize)
///
/// This is THE critical benchmark. It measures the complete manifest processing
/// time that each viewer incurs on every playlist request.
fn bench_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_pipeline");

    // Scenario A: Small live window (typical live)
    let small = generate_playlist(6, 1, 30.0);
    group.bench_with_input(
        BenchmarkId::new("scenario", "6seg_1break"),
        &small,
        |b, input| {
            b.iter(|| {
                full_pipeline(black_box(input));
            });
        },
    );

    // Scenario B: Medium live window
    let medium = generate_playlist(15, 1, 30.0);
    group.bench_with_input(
        BenchmarkId::new("scenario", "15seg_1break"),
        &medium,
        |b, input| {
            b.iter(|| {
                full_pipeline(black_box(input));
            });
        },
    );

    // Scenario C: Large window with multiple ad breaks (DVR/catchup)
    let large = generate_playlist(60, 3, 30.0);
    group.bench_with_input(
        BenchmarkId::new("scenario", "60seg_3breaks"),
        &large,
        |b, input| {
            b.iter(|| {
                full_pipeline(black_box(input));
            });
        },
    );

    // Scenario D: No ad breaks (pass-through)
    let no_ads = generate_playlist(12, 0, 0.0);
    group.bench_with_input(
        BenchmarkId::new("scenario", "12seg_0breaks"),
        &no_ads,
        |b, input| {
            b.iter(|| {
                full_pipeline(black_box(input));
            });
        },
    );

    group.finish();
}

/// Execute the full manifest stitching pipeline
fn full_pipeline(playlist_str: &str) -> String {
    // Step 1: Parse
    let playlist = parser::parse_hls_playlist(playlist_str).unwrap();

    let Playlist::MediaPlaylist(mut media) = playlist else {
        return parser::serialize_playlist(playlist).unwrap();
    };

    // Step 2: Detect ad breaks
    let ad_breaks = cue::detect_ad_breaks(&media);

    // Step 3: Interleave ads (using mock ad segments)
    if !ad_breaks.is_empty() {
        let ad_segments: Vec<Vec<AdSegment>> = ad_breaks
            .iter()
            .map(|ab| generate_ad_segments(ab.duration, 6.0))
            .collect();

        media = interleaver::interleave_ads(
            media,
            &ad_breaks,
            &ad_segments,
            "bench-session",
            "http://stitcher.example.com",
        );
    }

    // Step 4: Rewrite URLs
    let playlist = Playlist::MediaPlaylist(media);
    let rewritten = parser::rewrite_content_urls(
        playlist,
        "bench-session",
        "http://stitcher.example.com",
        "https://cdn.example.com/stream",
    )
    .unwrap();

    // Step 5: Serialize
    parser::serialize_playlist(rewritten).unwrap()
}

criterion_group!(
    benches,
    bench_parse_playlist,
    bench_detect_cue_breaks,
    bench_interleave_ads,
    bench_rewrite_urls,
    bench_rewrite_master,
    bench_serialize,
    bench_full_pipeline,
);
criterion_main!(benches);
