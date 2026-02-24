use crate::error::RitcherError;
use std::net::{Ipv4Addr, Ipv6Addr};
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
                return Err(RitcherError::InvalidOrigin(format!(
                    "Private or reserved IPv4 address not allowed: {ip}"
                )));
            }
        }
        Host::Ipv6(ip) => {
            if is_blocked_ipv6(ip) {
                return Err(RitcherError::InvalidOrigin(format!(
                    "Private or reserved IPv6 address not allowed: {ip}"
                )));
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
/// - `0.0.0.0/8`      — "this" network (RFC 1122)
/// - `10.0.0.0/8`     — RFC 1918 private
/// - `127.0.0.0/8`    — loopback
/// - `169.254.0.0/16` — link-local / cloud-metadata (AWS, GCP, Azure)
/// - `172.16.0.0/12`  — RFC 1918 private
/// - `192.168.0.0/16` — RFC 1918 private
fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    let (a, b) = (octets[0], octets[1]);

    a == 0                               // 0.0.0.0/8
        || a == 10                       // 10.0.0.0/8
        || a == 127                      // 127.0.0.0/8 loopback
        || (a == 169 && b == 254)        // 169.254.0.0/16 link-local
        || (a == 172 && (16..=31).contains(&b)) // 172.16.0.0/12
        || (a == 192 && b == 168) // 192.168.0.0/16
}

/// Returns `true` for IPv6 addresses in private or reserved ranges.
///
/// Blocked ranges:
/// - `::1/128`     — loopback
/// - `fe80::/10`   — link-local
/// - `fc00::/7`    — unique-local (ULA)
fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();

    ip.is_loopback()                     // ::1
        || (s[0] & 0xffc0) == 0xfe80    // fe80::/10 link-local
        || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
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
        assert!(validate_origin_url("https://203.0.113.1/stream").is_ok());
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
}
