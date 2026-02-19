//! Benchmarks for VAST XML parsing
//!
//! Tests parsing performance for different VAST response sizes and complexities.
//! VAST parsing happens on every ad break for every viewer, so its speed directly
//! impacts manifest latency during ad transitions.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use ritcher::ad::vast;

/// Generate a VAST InLine XML response with configurable number of ads and media files
fn generate_vast_inline(ad_count: usize, media_files_per_ad: usize) -> String {
    let mut xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">"#
        .to_string();

    for ad_idx in 0..ad_count {
        xml.push_str(&format!(
            r#"
  <Ad id="ad-{:03}">
    <InLine>
      <AdSystem>Benchmark Adserver</AdSystem>
      <AdTitle>Benchmark Ad {}</AdTitle>
      <Impression><![CDATA[https://tracking.example.com/impression?ad={}]]></Impression>
      <Creatives>
        <Creative id="creative-{:03}">
          <Linear>
            <Duration>00:00:15</Duration>
            <TrackingEvents>
              <Tracking event="start"><![CDATA[https://tracking.example.com/start?ad={}]]></Tracking>
              <Tracking event="firstQuartile"><![CDATA[https://tracking.example.com/q1?ad={}]]></Tracking>
              <Tracking event="midpoint"><![CDATA[https://tracking.example.com/mid?ad={}]]></Tracking>
              <Tracking event="thirdQuartile"><![CDATA[https://tracking.example.com/q3?ad={}]]></Tracking>
              <Tracking event="complete"><![CDATA[https://tracking.example.com/complete?ad={}]]></Tracking>
            </TrackingEvents>
            <MediaFiles>"#,
            ad_idx, ad_idx, ad_idx, ad_idx, ad_idx, ad_idx, ad_idx, ad_idx, ad_idx
        ));

        let resolutions = [
            (640, 360, 800, "video/mp4", "progressive"),
            (854, 480, 1400, "video/mp4", "progressive"),
            (1280, 720, 2800, "video/mp4", "progressive"),
            (1920, 1080, 5000, "video/mp4", "progressive"),
            (1280, 720, 0, "application/x-mpegURL", "streaming"),
        ];

        for mf_idx in 0..media_files_per_ad {
            let (w, h, br, mime, delivery) = resolutions[mf_idx % resolutions.len()];
            let bitrate_attr = if br > 0 {
                format!(" bitrate=\"{}\"", br)
            } else {
                String::new()
            };
            xml.push_str(&format!(
                r#"
              <MediaFile delivery="{}" type="{}" width="{}" height="{}"{} codec="H.264">
                <![CDATA[https://ads-cdn.example.com/creatives/ad_{:03}_{}x{}.{}]]>
              </MediaFile>"#,
                delivery,
                mime,
                w,
                h,
                bitrate_attr,
                ad_idx,
                w,
                h,
                if mime.contains("mpegURL") {
                    "m3u8"
                } else {
                    "mp4"
                }
            ));
        }

        xml.push_str(
            r#"
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>"#,
        );
    }

    xml.push_str("\n</VAST>");
    xml
}

/// Generate a VAST wrapper chain XML
fn generate_vast_wrapper() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
  <Ad id="wrapper-001">
    <Wrapper>
      <AdSystem>Wrapper Exchange</AdSystem>
      <VASTAdTagURI><![CDATA[https://exchange.example.com/vast?auction=12345&cb=67890]]></VASTAdTagURI>
      <Impression><![CDATA[https://tracking.example.com/wrapper-impression?id=001]]></Impression>
      <Creatives>
        <Creative>
          <Linear>
            <TrackingEvents>
              <Tracking event="start"><![CDATA[https://tracking.example.com/wrapper-start]]></Tracking>
              <Tracking event="complete"><![CDATA[https://tracking.example.com/wrapper-complete]]></Tracking>
            </TrackingEvents>
          </Linear>
        </Creative>
      </Creatives>
    </Wrapper>
  </Ad>
</VAST>"#
        .to_string()
}

/// Generate an empty VAST response (common for no-fill scenarios)
fn generate_vast_empty() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
</VAST>"#
        .to_string()
}

// ── Benchmarks ──────────────────────────────────────────────────────

/// Benchmark: Parse VAST InLine with varying ad count
fn bench_parse_vast_inline(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_vast_inline");

    for ad_count in [1, 3, 5, 10] {
        let xml = generate_vast_inline(ad_count, 3);

        group.bench_with_input(BenchmarkId::new("ads", ad_count), &xml, |b, input| {
            b.iter(|| {
                vast::parse_vast(black_box(input)).unwrap();
            });
        });
    }

    group.finish();
}

/// Benchmark: Parse VAST with varying media files per ad
fn bench_parse_vast_media_files(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_vast_media_files");

    for mf_count in [1, 3, 5] {
        let xml = generate_vast_inline(1, mf_count);

        group.bench_with_input(
            BenchmarkId::new("media_files", mf_count),
            &xml,
            |b, input| {
                b.iter(|| {
                    vast::parse_vast(black_box(input)).unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Parse VAST wrapper (redirect chain entry point)
fn bench_parse_vast_wrapper(c: &mut Criterion) {
    let xml = generate_vast_wrapper();

    c.bench_with_input(
        BenchmarkId::new("parse_vast_wrapper", "single"),
        &xml,
        |b, input| {
            b.iter(|| {
                vast::parse_vast(black_box(input)).unwrap();
            });
        },
    );
}

/// Benchmark: Parse empty VAST (no-fill response)
fn bench_parse_vast_empty(c: &mut Criterion) {
    let xml = generate_vast_empty();

    c.bench_with_input(
        BenchmarkId::new("parse_vast_empty", "no_fill"),
        &xml,
        |b, input| {
            b.iter(|| {
                vast::parse_vast(black_box(input)).unwrap();
            });
        },
    );
}

/// Benchmark: Select best media file from parsed VAST
fn bench_select_media_file(c: &mut Criterion) {
    let xml = generate_vast_inline(1, 5);
    let parsed = vast::parse_vast(&xml).unwrap();
    let ad = &parsed.ads[0];

    let media_files = match &ad.ad_type {
        vast::VastAdType::InLine(inline) => {
            &inline.creatives[0].linear.as_ref().unwrap().media_files
        }
        _ => panic!("Expected InLine"),
    };

    c.bench_with_input(
        BenchmarkId::new("select_best_media_file", "5_files"),
        media_files,
        |b, input| {
            b.iter(|| {
                vast::select_best_media_file(black_box(input));
            });
        },
    );
}

/// Benchmark: Realistic ad pod parsing (3 ads with 3 media files each)
///
/// Simulates a typical VAST response for a 90-second ad break with
/// three 30-second ads, each offering multiple quality renditions.
fn bench_parse_vast_realistic_pod(c: &mut Criterion) {
    let xml = generate_vast_inline(3, 3);
    let xml_size = xml.len();

    c.bench_with_input(
        BenchmarkId::new(
            "parse_vast_realistic",
            format!("3ads_3mf_{}bytes", xml_size),
        ),
        &xml,
        |b, input| {
            b.iter(|| {
                vast::parse_vast(black_box(input)).unwrap();
            });
        },
    );
}

criterion_group!(
    benches,
    bench_parse_vast_inline,
    bench_parse_vast_media_files,
    bench_parse_vast_wrapper,
    bench_parse_vast_empty,
    bench_select_media_file,
    bench_parse_vast_realistic_pod,
);
criterion_main!(benches);
