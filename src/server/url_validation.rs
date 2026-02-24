use crate::error::RitcherError;
use std::net::{Ipv4Addr, Ipv6Addr};
use tracing::warn;
use url::{Host, Url};

/// Validate that an origin URL is safe to fetch (SSRF protection).
///
/// Accepts only `http://` and `https://` URLs with a non-private host.
///
/// **IP literals** are checked against blocked ranges.
/// **Hostnames** are accepted without DNS resolution — DNS rebinding is a
/// known limitation accepted here; full mitigation requires async DNS lookup.
///
/// # Errors
/// Returns [`RitcherError::InvalidOrigin`] for:
/// - Invalid or relative URLs
/// - Non-HTTP(S) schemes
/// - IPv4 addresses in private/reserved ranges
/// - IPv6 loopback or link-local/unique-local addresses
pub fn validate_origin_url(url: &str) -> Result<(), RitcherError> {
    let parsed =
        Url::parse(url).map_err(|_| RitcherError::InvalidOrigin(format!("Invalid URL: {url}")))?;

    // Only allow HTTP(S)
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(RitcherError::InvalidOrigin(format!(
                "Scheme '{scheme}' not allowed — only http/https permitted"
            )));
        }
    }

    // Require a host
    let host = parsed
        .host()
        .ok_or_else(|| RitcherError::InvalidOrigin(format!("No host in URL: {url}")))?;

    match host {
        Host::Ipv4(ip) => {
            if is_blocked_ipv4(ip) {
                warn!("SSRF: blocked IPv4 origin {ip} from {url}");
                return Err(RitcherError::InvalidOrigin(
                    "Origin address is not allowed".to_string(),
                ));
            }
        }
        Host::Ipv6(ip) => {
            // Check native IPv6 ranges first
            if is_blocked_ipv6(ip) {
                warn!("SSRF: blocked IPv6 origin {ip} from {url}");
                return Err(RitcherError::InvalidOrigin(
                    "Origin address is not allowed".to_string(),
                ));
            }
            // Check for IPv4-mapped/compatible/NAT64 bypass vectors
            if let Some(embedded_v4) = extract_embedded_ipv4(ip)
                && is_blocked_ipv4(embedded_v4)
            {
                warn!(
                    "SSRF: blocked IPv4-mapped IPv6 origin {ip} (embedded {embedded_v4}) from {url}"
                );
                return Err(RitcherError::InvalidOrigin(
                    "Origin address is not allowed".to_string(),
                ));
            }
        }
        // Hostnames are allowed — we cannot resolve them without async DNS
        Host::Domain(_) => {}
    }

    Ok(())
}

/// Returns `true` for IPv4 addresses in private or reserved ranges.
///
/// Blocked ranges:
/// - `0.0.0.0/8`       — "this" network (RFC 1122)
/// - `10.0.0.0/8`      — RFC 1918 private
/// - `100.64.0.0/10`   — RFC 6598 CGNAT (cloud-internal)
/// - `127.0.0.0/8`     — loopback
/// - `169.254.0.0/16`  — link-local / cloud-metadata (AWS, GCP, Azure)
/// - `172.16.0.0/12`   — RFC 1918 private
/// - `192.0.0.0/24`    — IETF protocol assignments (RFC 6890)
/// - `192.0.2.0/24`    — TEST-NET-1 (RFC 5737)
/// - `192.168.0.0/16`  — RFC 1918 private
/// - `198.18.0.0/15`   — benchmarking (RFC 2544)
/// - `198.51.100.0/24` — TEST-NET-2 (RFC 5737)
/// - `203.0.113.0/24`  — TEST-NET-3 (RFC 5737)
/// - `240.0.0.0/4`     — reserved / Class E (RFC 1112)
/// - `255.255.255.255`  — broadcast
fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let (a, b, c) = (o[0], o[1], o[2]);

    a == 0                                       // 0.0.0.0/8
        || a == 10                               // 10.0.0.0/8
        || (a == 100 && (b & 0xC0) == 64)        // 100.64.0.0/10 CGNAT
        || a == 127                              // 127.0.0.0/8 loopback
        || (a == 169 && b == 254)                // 169.254.0.0/16 link-local
        || (a == 172 && (16..=31).contains(&b))  // 172.16.0.0/12
        || (a == 192 && b == 0 && c == 0)        // 192.0.0.0/24 IETF
        || (a == 192 && b == 0 && c == 2)        // 192.0.2.0/24 TEST-NET-1
        || (a == 192 && b == 168)                // 192.168.0.0/16
        || (a == 198 && (b & 0xFE) == 18)        // 198.18.0.0/15 benchmarking
        || (a == 198 && b == 51 && c == 100)     // 198.51.100.0/24 TEST-NET-2
        || (a == 203 && b == 0 && c == 113)      // 203.0.113.0/24 TEST-NET-3
        || a >= 240 // 240.0.0.0/4 reserved + broadcast
}

/// Returns `true` for IPv6 addresses in private or reserved ranges.
///
/// Blocked ranges:
/// - `::`          — unspecified (INADDR_ANY)
/// - `::1/128`     — loopback
/// - `fe80::/10`   — link-local
/// - `fc00::/7`    — unique-local (ULA)
/// - `2001:db8::/32` — documentation (RFC 3849)
///
/// IPv4-mapped/compatible/NAT64 addresses are handled separately via
/// [`extract_embedded_ipv4`].
fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();

    ip.is_unspecified()                  // ::
        || ip.is_loopback()              // ::1
        || (s[0] & 0xffc0) == 0xfe80    // fe80::/10 link-local
        || (s[0] & 0xfe00) == 0xfc00    // fc00::/7 unique-local
        || (s[0] == 0x2001 && s[1] == 0x0db8) // 2001:db8::/32 documentation
}

/// Extract an embedded IPv4 address from IPv6 transitional formats.
///
/// Catches bypass vectors where attackers use IPv6 notation to smuggle
/// private IPv4 addresses past the validator:
/// - `::ffff:x.x.x.x` — IPv4-mapped (RFC 4291)
/// - `::x.x.x.x`      — IPv4-compatible (deprecated, RFC 4291)
/// - `64:ff9b::/96`    — NAT64 well-known prefix (RFC 6052)
/// - `64:ff9b:1::/48`  — NAT64 local-use prefix (RFC 8215)
fn extract_embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segs = ip.segments();
    let bytes = ip.octets();

    // ::ffff:x.x.x.x (IPv4-mapped)
    if segs[0..5] == [0; 5] && segs[5] == 0xffff {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }

    // ::x.x.x.x (IPv4-compatible, deprecated) — all zeros except last 32 bits,
    // but not :: (unspecified) or ::1 (loopback) which are native IPv6.
    if segs[0..6] == [0; 6] && (segs[6] != 0 || segs[7] > 1) {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }

    // 64:ff9b::/96 (NAT64 well-known) and 64:ff9b:1::/48 (NAT64 local-use)
    if segs[0] == 0x0064 && segs[1] == 0xff9b {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- IPv4 private ranges ---

    #[test]
    fn test_rejects_localhost_127() {
        assert!(validate_origin_url("http://127.0.0.1/stream").is_err());
        assert!(validate_origin_url("http://127.0.0.99/stream").is_err());
        assert!(validate_origin_url("http://127.255.255.255/stream").is_err());
    }

    #[test]
    fn test_rejects_rfc1918_10() {
        assert!(validate_origin_url("http://10.0.0.1/stream").is_err());
        assert!(validate_origin_url("http://10.255.255.255/stream").is_err());
    }

    #[test]
    fn test_rejects_rfc1918_172() {
        assert!(validate_origin_url("http://172.16.0.1/stream").is_err());
        assert!(validate_origin_url("http://172.31.255.255/stream").is_err());
    }

    #[test]
    fn test_rejects_rfc1918_192_168() {
        assert!(validate_origin_url("http://192.168.0.1/stream").is_err());
        assert!(validate_origin_url("http://192.168.255.255/stream").is_err());
    }

    #[test]
    fn test_rejects_link_local_metadata() {
        // AWS/GCP/Azure cloud-metadata endpoint
        assert!(validate_origin_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_origin_url("http://169.254.0.1/stream").is_err());
    }

    #[test]
    fn test_rejects_zero_network() {
        assert!(validate_origin_url("http://0.0.0.0/stream").is_err());
        assert!(validate_origin_url("http://0.1.2.3/stream").is_err());
    }

    // --- IPv6 private ranges ---

    #[test]
    fn test_rejects_ipv6_loopback() {
        assert!(validate_origin_url("http://[::1]/stream").is_err());
    }

    #[test]
    fn test_rejects_ipv6_link_local() {
        assert!(validate_origin_url("http://[fe80::1]/stream").is_err());
        assert!(validate_origin_url("http://[fe80::abcd:1234]/stream").is_err());
    }

    #[test]
    fn test_rejects_ipv6_unique_local() {
        assert!(validate_origin_url("http://[fc00::1]/stream").is_err());
        assert!(validate_origin_url("http://[fd00::1]/stream").is_err());
        assert!(validate_origin_url("http://[fdff:ffff::1]/stream").is_err());
    }

    // --- Public addresses allowed ---

    #[test]
    fn test_allows_public_ipv4() {
        assert!(validate_origin_url("http://1.2.3.4/stream").is_ok());
        assert!(validate_origin_url("https://8.8.8.8/dns").is_ok());
        assert!(validate_origin_url("https://93.184.216.34/stream").is_ok());
    }

    #[test]
    fn test_allows_public_hostname() {
        assert!(validate_origin_url("https://cdn.example.com/stream.m3u8").is_ok());
        assert!(validate_origin_url("http://live.broadcaster.com/playlist.m3u8").is_ok());
    }

    // --- Scheme validation ---

    #[test]
    fn test_rejects_ftp_scheme() {
        assert!(validate_origin_url("ftp://cdn.example.com/file.ts").is_err());
    }

    #[test]
    fn test_rejects_file_scheme() {
        assert!(validate_origin_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_rejects_gopher_scheme() {
        assert!(validate_origin_url("gopher://cdn.example.com/stream").is_err());
    }

    #[test]
    fn test_rejects_no_scheme() {
        assert!(validate_origin_url("cdn.example.com/stream").is_err());
    }

    // --- Malformed / edge cases ---

    #[test]
    fn test_rejects_empty_url() {
        assert!(validate_origin_url("").is_err());
    }

    #[test]
    fn test_rejects_garbage() {
        assert!(validate_origin_url("not-a-url").is_err());
        assert!(validate_origin_url("://missing-scheme").is_err());
    }

    // --- Range boundary tests ---

    #[test]
    fn test_boundary_172_15_not_blocked() {
        // 172.15.x.x is just outside the 172.16.0.0/12 range
        assert!(validate_origin_url("http://172.15.255.255/stream").is_ok());
    }

    #[test]
    fn test_boundary_172_32_not_blocked() {
        // 172.32.x.x is just outside the 172.16.0.0/12 range
        assert!(validate_origin_url("http://172.32.0.0/stream").is_ok());
    }

    #[test]
    fn test_allows_https_with_path_and_query() {
        assert!(validate_origin_url("https://cdn.example.com/live/stream.m3u8?token=abc").is_ok());
    }

    // --- CGNAT (RFC 6598) ---

    #[test]
    fn test_rejects_cgnat_100_64() {
        assert!(validate_origin_url("http://100.64.0.1/stream").is_err());
        assert!(validate_origin_url("http://100.127.255.255/stream").is_err());
    }

    #[test]
    fn test_boundary_cgnat_100_63_allowed() {
        assert!(validate_origin_url("http://100.63.255.255/stream").is_ok());
    }

    #[test]
    fn test_boundary_cgnat_100_128_allowed() {
        assert!(validate_origin_url("http://100.128.0.0/stream").is_ok());
    }

    // --- IETF / TEST-NET / benchmarking / Class E ---

    #[test]
    fn test_rejects_ietf_192_0_0() {
        assert!(validate_origin_url("http://192.0.0.1/stream").is_err());
    }

    #[test]
    fn test_rejects_test_net_1() {
        assert!(validate_origin_url("http://192.0.2.1/stream").is_err());
    }

    #[test]
    fn test_rejects_test_net_2() {
        assert!(validate_origin_url("http://198.51.100.1/stream").is_err());
    }

    #[test]
    fn test_rejects_test_net_3() {
        assert!(validate_origin_url("http://203.0.113.1/stream").is_err());
    }

    #[test]
    fn test_rejects_benchmarking_198_18() {
        assert!(validate_origin_url("http://198.18.0.1/stream").is_err());
        assert!(validate_origin_url("http://198.19.255.255/stream").is_err());
    }

    #[test]
    fn test_rejects_class_e_240() {
        assert!(validate_origin_url("http://240.0.0.1/stream").is_err());
        assert!(validate_origin_url("http://255.255.255.255/stream").is_err());
    }

    // --- IPv4-mapped/compatible IPv6 bypass vectors ---

    #[test]
    fn test_rejects_ipv4_mapped_loopback() {
        // ::ffff:127.0.0.1 — bypass attempt via IPv4-mapped IPv6
        assert!(validate_origin_url("http://[::ffff:127.0.0.1]/stream").is_err());
    }

    #[test]
    fn test_rejects_ipv4_mapped_metadata() {
        // ::ffff:169.254.169.254 — cloud metadata bypass
        assert!(validate_origin_url("http://[::ffff:169.254.169.254]/stream").is_err());
    }

    #[test]
    fn test_rejects_ipv4_mapped_private() {
        assert!(validate_origin_url("http://[::ffff:10.0.0.1]/stream").is_err());
        assert!(validate_origin_url("http://[::ffff:192.168.1.1]/stream").is_err());
    }

    #[test]
    fn test_allows_ipv4_mapped_public() {
        assert!(validate_origin_url("http://[::ffff:8.8.8.8]/stream").is_ok());
    }

    // --- IPv6 unspecified and documentation ---

    #[test]
    fn test_rejects_ipv6_unspecified() {
        assert!(validate_origin_url("http://[::]/stream").is_err());
    }

    #[test]
    fn test_rejects_ipv6_documentation() {
        assert!(validate_origin_url("http://[2001:db8::1]/stream").is_err());
    }

    // --- Generic error message (no IP leak) ---

    #[test]
    fn test_error_message_does_not_leak_ip() {
        let err = validate_origin_url("http://127.0.0.1/stream").unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("127.0.0.1"),
            "Error message should not contain the blocked IP, got: {msg}"
        );
        assert!(
            msg.contains("not allowed"),
            "Error should contain generic message, got: {msg}"
        );
    }
}
