//! SSRF-safe DNS resolver for reqwest.
//!
//! Wraps `hickory_resolver::Resolver` to validate every resolved IP address
//! against the SSRF blocklist before handing the addresses to reqwest for TCP
//! connection. This eliminates the TOCTOU gap where a hostname could resolve
//! to a private IP between URL validation and connection time (DNS rebinding).

use crate::server::url_validation::{extract_embedded_ipv4, is_blocked_ipv4, is_blocked_ipv6};
use hickory_resolver::TokioResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tracing::warn;

/// A DNS resolver that filters out private/reserved IP addresses.
///
/// Implements [`reqwest::dns::Resolve`] so it can be plugged into
/// `reqwest::ClientBuilder::dns_resolver()`. For every DNS lookup it:
///
/// 1. Resolves the hostname via the system's DNS configuration (using hickory-dns).
/// 2. Filters each resolved IP against the SSRF blocklist (`is_blocked_ipv4`,
///    `is_blocked_ipv6`, `extract_embedded_ipv4`).
/// 3. Returns only the safe IPs. If all IPs are blocked, returns an error.
pub struct SsrfSafeResolver {
    resolver: Arc<TokioResolver>,
}

impl Default for SsrfSafeResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SsrfSafeResolver {
    /// Create a new SSRF-safe resolver using the system's DNS configuration.
    ///
    /// # Panics
    /// Panics if the system DNS configuration cannot be loaded. This is called
    /// once at startup, so a panic is acceptable (fail-fast on misconfiguration).
    pub fn new() -> Self {
        let resolver = TokioResolver::builder_tokio()
            .expect("Failed to read system DNS configuration")
            .build();
        Self {
            resolver: Arc::new(resolver),
        }
    }
}

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.resolver.clone();
        Box::pin(async move {
            let hostname = name.as_str();

            let lookup = resolver.lookup_ip(hostname).await.map_err(|e| {
                Box::new(io::Error::other(format!(
                    "DNS lookup failed for {hostname}: {e}"
                ))) as Box<dyn std::error::Error + Send + Sync>
            })?;

            let safe_addrs: Vec<SocketAddr> = lookup
                .iter()
                .filter(is_safe_ip)
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();

            if safe_addrs.is_empty() {
                warn!(
                    "SSRF: DNS rebinding blocked -- all resolved IPs for {hostname} are private/reserved"
                );
                return Err(Box::new(io::Error::other(format!(
                    "All resolved addresses for {hostname} are blocked by SSRF policy"
                )))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            let addrs: Addrs = Box::new(safe_addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// Check whether an IP address is safe (not in any blocked range).
///
/// Returns `true` if the IP is allowed, `false` if it should be blocked.
fn is_safe_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !is_blocked_ipv4(*v4),
        IpAddr::V6(v6) => {
            if is_blocked_ipv6(*v6) {
                return false;
            }
            // Also check for IPv4-mapped/compatible/NAT64 embedded addresses
            if let Some(embedded_v4) = extract_embedded_ipv4(*v6) {
                return !is_blocked_ipv4(embedded_v4);
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    // --- is_safe_ip unit tests ---

    #[test]
    fn blocks_loopback_ipv4() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn blocks_rfc1918_10() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));
    }

    #[test]
    fn blocks_rfc1918_172() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
    }

    #[test]
    fn blocks_rfc1918_192_168() {
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, 255, 255))));
    }

    #[test]
    fn blocks_cloud_metadata_endpoint() {
        // AWS/GCP/Azure metadata at 169.254.169.254
        assert!(!is_safe_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
    }

    #[test]
    fn blocks_ipv6_loopback() {
        assert!(!is_safe_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn blocks_ipv6_link_local() {
        // fe80::1
        assert!(!is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn blocks_ipv6_unique_local() {
        // fd00::1
        assert!(!is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn blocks_ipv4_mapped_loopback() {
        // ::ffff:127.0.0.1
        assert!(!is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001
        ))));
    }

    #[test]
    fn blocks_ipv4_mapped_private() {
        // ::ffff:10.0.0.1
        assert!(!is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0xffff, 0x0a00, 0x0001
        ))));
        // ::ffff:192.168.1.1
        assert!(!is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0101
        ))));
    }

    #[test]
    fn allows_public_ipv4() {
        assert!(is_safe_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_safe_ip(&IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
        assert!(is_safe_ip(&IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn allows_public_ipv6() {
        // 2606:4700:4700::1111 (Cloudflare DNS)
        assert!(is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111
        ))));
    }

    #[test]
    fn allows_ipv4_mapped_public() {
        // ::ffff:8.8.8.8
        assert!(is_safe_ip(&IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0xffff, 0x0808, 0x0808
        ))));
    }

    // --- Resolver integration tests (require network) ---

    #[tokio::test]
    #[ignore] // Requires network access
    async fn resolver_allows_public_domain() {
        let resolver = SsrfSafeResolver::new();
        // example.com always resolves to a public IP (93.184.216.34)
        let name: Name = "example.com".parse().unwrap();
        let result = resolver.resolve(name);
        let addrs = result.await;
        assert!(addrs.is_ok(), "Public domain should resolve successfully");
        let addrs: Vec<SocketAddr> = addrs.unwrap().collect();
        assert!(!addrs.is_empty(), "Should have at least one address");
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn resolver_handles_nonexistent_domain() {
        let resolver = SsrfSafeResolver::new();
        let name: Name = "this-domain-definitely-does-not-exist-ritcher-test.invalid"
            .parse()
            .unwrap();
        let result = resolver.resolve(name).await;
        assert!(result.is_err(), "Nonexistent domain should return error");
    }
}
