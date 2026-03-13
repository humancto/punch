//! SSRF (Server-Side Request Forgery) protection engine.
//!
//! Guards the ring against fighters that try to reach out to internal
//! network resources they have no business touching. The protector validates
//! URLs before any HTTP request lands, blocking private IP ranges, dangerous
//! schemes, and DNS rebinding attacks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SSRF violation
// ---------------------------------------------------------------------------

/// Describes how a URL violated SSRF protection rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SsrfViolation {
    /// The resolved IP address falls within a blocked CIDR range.
    BlockedIp { ip: String, range: String },
    /// The URL scheme is not allowed (e.g., `file://`, `ftp://`).
    BlockedScheme { scheme: String },
    /// The hostname is explicitly blocked.
    BlockedHost { host: String },
    /// DNS resolution failed for the hostname.
    DnsResolutionFailed { host: String, reason: String },
    /// The resolved IP is in a private/reserved range.
    PrivateIp { ip: String },
    /// The URL matched a custom blocked pattern.
    BlockedPattern { pattern: String, url: String },
    /// The URL could not be parsed.
    InvalidUrl { reason: String },
}

impl std::fmt::Display for SsrfViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockedIp { ip, range } => {
                write!(f, "SSRF: IP {} falls within blocked range {}", ip, range)
            }
            Self::BlockedScheme { scheme } => {
                write!(f, "SSRF: scheme '{}' is not allowed", scheme)
            }
            Self::BlockedHost { host } => {
                write!(f, "SSRF: hostname '{}' is blocked", host)
            }
            Self::DnsResolutionFailed { host, reason } => {
                write!(f, "SSRF: DNS resolution failed for '{}': {}", host, reason)
            }
            Self::PrivateIp { ip } => {
                write!(f, "SSRF: resolved IP {} is in a private range", ip)
            }
            Self::BlockedPattern { pattern, url } => {
                write!(
                    f,
                    "SSRF: URL '{}' matched blocked pattern '{}'",
                    url, pattern
                )
            }
            Self::InvalidUrl { reason } => {
                write!(f, "SSRF: invalid URL: {}", reason)
            }
        }
    }
}

impl std::error::Error for SsrfViolation {}

// ---------------------------------------------------------------------------
// CIDR range (simple implementation)
// ---------------------------------------------------------------------------

/// A CIDR range for IP matching.
#[derive(Debug, Clone)]
struct CidrRange {
    /// Human-readable description (e.g., "127.0.0.0/8").
    label: String,
    /// The network address.
    network: IpAddr,
    /// Prefix length in bits.
    prefix_len: u8,
}

impl CidrRange {
    fn contains(&self, ip: &IpAddr) -> bool {
        match (&self.network, ip) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => {
                let net_bits = u32::from(*net);
                let addr_bits = u32::from(*addr);
                if self.prefix_len == 0 {
                    return true;
                }
                if self.prefix_len >= 32 {
                    return net_bits == addr_bits;
                }
                let mask = !((1u32 << (32 - self.prefix_len)) - 1);
                (net_bits & mask) == (addr_bits & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(addr)) => {
                let net_bits = u128::from(*net);
                let addr_bits = u128::from(*addr);
                if self.prefix_len == 0 {
                    return true;
                }
                if self.prefix_len >= 128 {
                    return net_bits == addr_bits;
                }
                let mask = !((1u128 << (128 - self.prefix_len)) - 1);
                (net_bits & mask) == (addr_bits & mask)
            }
            _ => false,
        }
    }
}

fn parse_cidr(s: &str) -> Option<CidrRange> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let ip: IpAddr = parts[0].parse().ok()?;
    let prefix_len: u8 = parts[1].parse().ok()?;
    Some(CidrRange {
        label: s.to_string(),
        network: ip,
        prefix_len,
    })
}

// ---------------------------------------------------------------------------
// SsrfProtector
// ---------------------------------------------------------------------------

/// The SSRF protection engine — validates URLs before they leave the ring.
///
/// Blocks requests to private IP ranges, dangerous schemes, and specific
/// hostnames. Supports allow-listing for trusted internal hosts and custom
/// regex-based blocking patterns.
#[derive(Debug, Clone)]
pub struct SsrfProtector {
    /// CIDR ranges to block.
    blocked_ranges: Vec<CidrRange>,
    /// Hostnames to block.
    blocked_hosts: Vec<String>,
    /// URL schemes to block.
    blocked_schemes: Vec<String>,
    /// Hostnames explicitly allowed (bypass IP checks).
    allowed_hosts: Vec<String>,
    /// Custom regex patterns to block.
    blocked_patterns: Vec<(String, Regex)>,
    /// Whether to perform DNS resolution checks.
    dns_check_enabled: bool,
}

impl Default for SsrfProtector {
    fn default() -> Self {
        Self::new()
    }
}

impl SsrfProtector {
    /// Create a new protector with default blocked ranges and schemes.
    pub fn new() -> Self {
        let default_cidrs = [
            "127.0.0.0/8",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "169.254.0.0/16",
            "::1/128",
            "fc00::/7",
            "fe80::/10",
        ];

        let blocked_ranges: Vec<CidrRange> =
            default_cidrs.iter().filter_map(|c| parse_cidr(c)).collect();

        Self {
            blocked_ranges,
            blocked_hosts: vec![
                "localhost".to_string(),
                "metadata.google.internal".to_string(),
                "169.254.169.254".to_string(),
            ],
            blocked_schemes: vec!["file".to_string(), "ftp".to_string(), "gopher".to_string()],
            allowed_hosts: Vec::new(),
            blocked_patterns: Vec::new(),
            dns_check_enabled: true,
        }
    }

    /// Add a hostname to the allow-list (bypasses IP range checks).
    pub fn allow_host(&mut self, host: &str) {
        self.allowed_hosts.push(host.to_lowercase());
    }

    /// Add a custom regex pattern to block.
    pub fn add_blocked_pattern(&mut self, name: &str, pattern: &str) {
        if let Ok(re) = Regex::new(pattern) {
            self.blocked_patterns.push((name.to_string(), re));
        }
    }

    /// Add a custom CIDR range to block.
    pub fn add_blocked_range(&mut self, cidr: &str) {
        if let Some(range) = parse_cidr(cidr) {
            self.blocked_ranges.push(range);
        }
    }

    /// Block an additional hostname.
    pub fn block_host(&mut self, host: &str) {
        self.blocked_hosts.push(host.to_lowercase());
    }

    /// Enable or disable DNS resolution checks.
    pub fn set_dns_check(&mut self, enabled: bool) {
        self.dns_check_enabled = enabled;
    }

    /// Validate a URL, returning `Ok(())` if it is safe to request.
    pub fn validate_url(&self, url: &str) -> Result<(), SsrfViolation> {
        // Check custom blocked patterns first.
        for (name, re) in &self.blocked_patterns {
            if re.is_match(url) {
                return Err(SsrfViolation::BlockedPattern {
                    pattern: name.clone(),
                    url: url.to_string(),
                });
            }
        }

        // Parse scheme.
        let scheme = extract_scheme(url)?;
        if self.blocked_schemes.contains(&scheme.to_lowercase()) {
            return Err(SsrfViolation::BlockedScheme { scheme });
        }

        // Parse host.
        let host = extract_host(url)?;
        let host_lower = host.to_lowercase();

        // Check blocked hosts.
        if self.blocked_hosts.contains(&host_lower) {
            return Err(SsrfViolation::BlockedHost {
                host: host.to_string(),
            });
        }

        // If the host is explicitly allowed, skip IP checks.
        if self.allowed_hosts.contains(&host_lower) {
            return Ok(());
        }

        // Check if the host is a literal IP address.
        if let Ok(ip) = host.parse::<IpAddr>() {
            self.check_ip(&ip)?;
            return Ok(());
        }

        // DNS resolution check.
        if self.dns_check_enabled {
            self.check_dns(&host)?;
        }

        Ok(())
    }

    /// Check a resolved IP against blocked ranges.
    fn check_ip(&self, ip: &IpAddr) -> Result<(), SsrfViolation> {
        // Check if IP is private/reserved.
        if is_private_ip(ip) {
            return Err(SsrfViolation::PrivateIp { ip: ip.to_string() });
        }

        // Check explicit CIDR blocks.
        for range in &self.blocked_ranges {
            if range.contains(ip) {
                return Err(SsrfViolation::BlockedIp {
                    ip: ip.to_string(),
                    range: range.label.clone(),
                });
            }
        }

        Ok(())
    }

    /// Resolve the hostname and verify all resolved IPs are safe.
    fn check_dns(&self, host: &str) -> Result<(), SsrfViolation> {
        let addr_str = format!("{}:80", host);
        let addrs = addr_str
            .to_socket_addrs()
            .map_err(|e| SsrfViolation::DnsResolutionFailed {
                host: host.to_string(),
                reason: e.to_string(),
            })?;

        for addr in addrs {
            self.check_ip(&addr.ip())?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Check if an IP address is in a private/reserved range.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || *v4 == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_unique_local(v6)
                || is_ipv6_link_local(v6)
        }
    }
}

fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    // fc00::/7
    let first_byte = v6.octets()[0];
    (first_byte & 0xFE) == 0xFC
}

fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    // fe80::/10
    let octets = v6.octets();
    octets[0] == 0xFE && (octets[1] & 0xC0) == 0x80
}

/// Extract the scheme from a URL string.
fn extract_scheme(url: &str) -> Result<String, SsrfViolation> {
    if let Some(idx) = url.find("://") {
        Ok(url[..idx].to_string())
    } else {
        Err(SsrfViolation::InvalidUrl {
            reason: "missing scheme (no :// found)".into(),
        })
    }
}

/// Extract the hostname from a URL string.
fn extract_host(url: &str) -> Result<String, SsrfViolation> {
    let after_scheme =
        url.find("://")
            .map(|i| &url[i + 3..])
            .ok_or_else(|| SsrfViolation::InvalidUrl {
                reason: "missing scheme".into(),
            })?;

    // Strip userinfo (user:pass@).
    let after_userinfo = if let Some(at) = after_scheme.find('@') {
        &after_scheme[at + 1..]
    } else {
        after_scheme
    };

    // Handle IPv6 addresses in brackets.
    if after_userinfo.starts_with('[') {
        if let Some(end) = after_userinfo.find(']') {
            return Ok(after_userinfo[1..end].to_string());
        }
        return Err(SsrfViolation::InvalidUrl {
            reason: "unclosed bracket in IPv6 address".into(),
        });
    }

    // Take everything before the first : or / or ? or #.
    let host = after_userinfo
        .split([':', '/', '?', '#'])
        .next()
        .unwrap_or("");

    if host.is_empty() {
        return Err(SsrfViolation::InvalidUrl {
            reason: "empty hostname".into(),
        });
    }

    Ok(host.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn protector_no_dns() -> SsrfProtector {
        let mut p = SsrfProtector::new();
        p.set_dns_check(false);
        p
    }

    #[test]
    fn test_blocks_localhost() {
        let p = protector_no_dns();
        let result = p.validate_url("http://localhost/admin");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SsrfViolation::BlockedHost { .. }
        ));
    }

    #[test]
    fn test_blocks_127_0_0_1() {
        let p = protector_no_dns();
        let result = p.validate_url("http://127.0.0.1/admin");
        assert!(result.is_err());
        match result.unwrap_err() {
            SsrfViolation::PrivateIp { ip } | SsrfViolation::BlockedIp { ip, .. } => {
                assert!(ip.starts_with("127."));
            }
            other => panic!("expected PrivateIp or BlockedIp, got {:?}", other),
        }
    }

    #[test]
    fn test_blocks_10_x_private_range() {
        let p = protector_no_dns();
        let result = p.validate_url("http://10.0.0.1/internal");
        assert!(result.is_err());
    }

    #[test]
    fn test_blocks_172_16_private_range() {
        let p = protector_no_dns();
        let result = p.validate_url("http://172.16.0.1/secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_blocks_192_168_private_range() {
        let p = protector_no_dns();
        let result = p.validate_url("http://192.168.1.1/router");
        assert!(result.is_err());
    }

    #[test]
    fn test_blocks_link_local() {
        let p = protector_no_dns();
        let result = p.validate_url("http://169.254.169.254/latest/meta-data/");
        assert!(result.is_err());
    }

    #[test]
    fn test_blocks_ipv6_localhost() {
        let p = protector_no_dns();
        let result = p.validate_url("http://[::1]/admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_blocks_file_scheme() {
        let p = protector_no_dns();
        let result = p.validate_url("file:///etc/passwd");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SsrfViolation::BlockedScheme { .. }
        ));
    }

    #[test]
    fn test_blocks_ftp_scheme() {
        let p = protector_no_dns();
        let result = p.validate_url("ftp://internal-server/data");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SsrfViolation::BlockedScheme { .. }
        ));
    }

    #[test]
    fn test_blocks_gopher_scheme() {
        let p = protector_no_dns();
        let result = p.validate_url("gopher://evil.com/1");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SsrfViolation::BlockedScheme { .. }
        ));
    }

    #[test]
    fn test_allows_public_url() {
        let p = protector_no_dns();
        let result = p.validate_url("https://example.com/api");
        assert!(result.is_ok());
    }

    #[test]
    fn test_allows_explicit_allowed_host() {
        let mut p = protector_no_dns();
        p.allow_host("internal.mycompany.com");
        let result = p.validate_url("http://internal.mycompany.com/api");
        assert!(result.is_ok());
    }

    #[test]
    fn test_blocks_custom_pattern() {
        let mut p = protector_no_dns();
        p.add_blocked_pattern("aws_metadata", r"169\.254\.169\.254");
        let result = p.validate_url("http://169.254.169.254/latest/");
        assert!(result.is_err());
    }

    #[test]
    fn test_blocks_metadata_google_internal() {
        let p = protector_no_dns();
        let result = p.validate_url("http://metadata.google.internal/computeMetadata/v1/");
        assert!(result.is_err());
    }

    #[test]
    fn test_allows_public_ip() {
        let p = protector_no_dns();
        let result = p.validate_url("http://8.8.8.8/dns");
        assert!(result.is_ok());
    }

    #[test]
    fn test_invalid_url_no_scheme() {
        let p = protector_no_dns();
        let result = p.validate_url("just-a-hostname");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SsrfViolation::InvalidUrl { .. }
        ));
    }

    #[test]
    fn test_cidr_range_contains() {
        let range = parse_cidr("10.0.0.0/8").unwrap();
        assert!(range.contains(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(range.contains(&IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));
        assert!(!range.contains(&IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1))));
    }

    #[test]
    fn test_ipv6_unique_local_blocked() {
        let p = protector_no_dns();
        let result = p.validate_url("http://[fd00::1]/internal");
        assert!(result.is_err());
    }

    #[test]
    fn test_url_with_port() {
        let p = protector_no_dns();
        let result = p.validate_url("http://192.168.1.1:8080/api");
        assert!(result.is_err());
    }

    #[test]
    fn test_url_with_userinfo() {
        let p = protector_no_dns();
        let result = p.validate_url("http://admin:pass@10.0.0.1/secret");
        assert!(result.is_err());
    }
}
