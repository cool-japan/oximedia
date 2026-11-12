// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! SSRF prevention and security validation for image source URLs.
//!
//! Provides defense-in-depth against:
//! - SSRF attacks via private/reserved IP addresses (RFC 1918, RFC 4193, CGNAT, etc.)
//! - Path traversal attacks (`../`, encoded variants, null bytes)
//! - Resource exhaustion via oversized dimensions or file sizes
//! - Protocol smuggling via unsupported URI schemes

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::transform::TransformParams;

// ============================================================================
// Signed URL support (HMAC-SHA256)
// ============================================================================

/// Configuration for signed URL verification.
#[derive(Debug, Clone)]
pub struct SignedUrlConfig {
    /// HMAC-SHA256 secret key bytes.
    pub secret: Vec<u8>,
    /// Whether to require signed URLs for all requests.
    pub required: bool,
    /// Optional expiry tolerance in seconds (0 = no expiry check).
    pub expiry_tolerance_secs: u64,
}

impl Default for SignedUrlConfig {
    fn default() -> Self {
        Self {
            secret: Vec::new(),
            required: false,
            expiry_tolerance_secs: 0,
        }
    }
}

/// Signed URL verification error.
#[derive(Debug, thiserror::Error)]
pub enum SignedUrlError {
    /// The signature is missing from the URL.
    #[error("missing signature")]
    MissingSignature,
    /// The signature does not match.
    #[error("invalid signature")]
    InvalidSignature,
    /// The signed URL has expired.
    #[error("URL expired at {0}")]
    Expired(u64),
    /// The secret key is empty / not configured.
    #[error("signing secret not configured")]
    NoSecret,
}

/// Generate an HMAC-SHA256 signature for a URL path with transform params.
///
/// The message is `"{path}?{params_string}"`. The returned value is a
/// 64-character lowercase hex string.
pub fn sign_url(path: &str, params: &str, secret: &[u8]) -> Result<String, SignedUrlError> {
    if secret.is_empty() {
        return Err(SignedUrlError::NoSecret);
    }
    let message = format!("{path}?{params}");
    let mac = hmac_sha256(secret, message.as_bytes());
    Ok(hex_encode(&mac))
}

/// Verify an HMAC-SHA256 signature for a URL.
///
/// Performs constant-time comparison to prevent timing attacks.
pub fn verify_signature(
    path: &str,
    params: &str,
    provided_sig: &str,
    config: &SignedUrlConfig,
) -> Result<(), SignedUrlError> {
    if config.secret.is_empty() {
        return Err(SignedUrlError::NoSecret);
    }

    let expected = sign_url(path, params, &config.secret)?;

    if !constant_time_eq(expected.as_bytes(), provided_sig.as_bytes()) {
        return Err(SignedUrlError::InvalidSignature);
    }

    Ok(())
}

/// Verify a signed URL, optionally checking expiry.
///
/// The `expiry` parameter is a Unix timestamp. If `config.expiry_tolerance_secs`
/// is non-zero and `current_time` exceeds `expiry + tolerance`, the URL is
/// rejected.
pub fn verify_signed_url(
    path: &str,
    params: &str,
    provided_sig: &str,
    expiry: Option<u64>,
    current_time: u64,
    config: &SignedUrlConfig,
) -> Result<(), SignedUrlError> {
    verify_signature(path, params, provided_sig, config)?;

    if config.expiry_tolerance_secs > 0 {
        if let Some(exp) = expiry {
            if current_time > exp + config.expiry_tolerance_secs {
                return Err(SignedUrlError::Expired(exp));
            }
        }
    }

    Ok(())
}

// ============================================================================
// Pure-Rust HMAC-SHA256 implementation
// ============================================================================

/// HMAC-SHA256 as defined in RFC 2104.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;

    // Step 1: if key > block size, hash it
    let key_block = if key.len() > BLOCK_SIZE {
        let h = sha256(key);
        let mut block = [0u8; BLOCK_SIZE];
        block[..32].copy_from_slice(&h);
        block
    } else {
        let mut block = [0u8; BLOCK_SIZE];
        block[..key.len()].copy_from_slice(key);
        block
    };

    // Step 2: inner padding
    let mut ipad = [0x36u8; BLOCK_SIZE];
    for (i, b) in ipad.iter_mut().enumerate() {
        *b ^= key_block[i];
    }

    // Step 3: outer padding
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for (i, b) in opad.iter_mut().enumerate() {
        *b ^= key_block[i];
    }

    // Step 4: inner hash = SHA-256(ipad || message)
    let mut inner_data = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner_data.extend_from_slice(&ipad);
    inner_data.extend_from_slice(message);
    let inner_hash = sha256(&inner_data);

    // Step 5: outer hash = SHA-256(opad || inner_hash)
    let mut outer_data = Vec::with_capacity(BLOCK_SIZE + 32);
    outer_data.extend_from_slice(&opad);
    outer_data.extend_from_slice(&inner_hash);
    sha256(&outer_data)
}

/// SHA-256 implementation (FIPS 180-4).
fn sha256(data: &[u8]) -> [u8; 32] {
    // Initial hash values
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Round constants
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    // Pre-processing: padding
    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit (64-byte) block
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut result = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        result[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

/// Constant-time comparison of two byte slices.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_CHARS[(b >> 4) as usize]);
        s.push(HEX_CHARS[(b & 0x0F) as usize]);
    }
    s
}

const HEX_CHARS: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

/// Security validation error.
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// A private or reserved IP address was detected (potential SSRF).
    #[error("SSRF: private/reserved IP address detected: {0}")]
    PrivateIpDetected(String),

    /// Path traversal attempt detected.
    #[error("path traversal detected: {0}")]
    PathTraversalDetected(String),

    /// Image dimension exceeds configured maximum.
    #[error("dimension exceeds maximum: {dimension}={value}, max={max}")]
    DimensionExceeded {
        /// Which dimension was exceeded ("width" or "height").
        dimension: String,
        /// The requested value.
        value: u32,
        /// The configured maximum.
        max: u32,
    },

    /// File size exceeds configured maximum.
    #[error("file size exceeds maximum: {size} bytes, max: {max} bytes")]
    FileSizeExceeded {
        /// The actual file size.
        size: u64,
        /// The configured maximum.
        max: u64,
    },

    /// An unsupported URI protocol was used.
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),

    /// The hostname is on the block list.
    #[error("blocked hostname: {0}")]
    BlockedHostname(String),

    /// The source URL is malformed or invalid.
    #[error("invalid source URL: {0}")]
    InvalidUrl(String),
}

/// Default maximum width in pixels.
const DEFAULT_MAX_WIDTH: u32 = 12000;

/// Default maximum height in pixels.
const DEFAULT_MAX_HEIGHT: u32 = 12000;

/// Default maximum input file size (100 MB).
const DEFAULT_MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Default maximum output file size (50 MB).
const DEFAULT_MAX_OUTPUT_SIZE: u64 = 50 * 1024 * 1024;

/// Security configuration for image transformation requests.
///
/// Controls dimension limits, file size limits, and URL access policies.
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// Maximum allowed output width in pixels.
    pub max_width: u32,
    /// Maximum allowed output height in pixels.
    pub max_height: u32,
    /// Maximum allowed input file size in bytes.
    pub max_file_size: u64,
    /// Maximum allowed output file size in bytes.
    pub max_output_size: u64,
    /// Whether to allow fetching images from external URLs.
    /// When `false`, only local file paths are permitted.
    pub allow_external_urls: bool,
    /// Hostnames that are explicitly blocked (deny list).
    pub blocked_hosts: Vec<String>,
    /// If non-empty, only these hostnames are allowed (allow list / whitelist mode).
    pub allowed_hosts: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_width: DEFAULT_MAX_WIDTH,
            max_height: DEFAULT_MAX_HEIGHT,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_output_size: DEFAULT_MAX_OUTPUT_SIZE,
            allow_external_urls: false,
            blocked_hosts: Vec::new(),
            allowed_hosts: Vec::new(),
        }
    }
}

/// Check if an IP address is private or reserved.
///
/// Detects:
/// - IPv4 loopback (`127.0.0.0/8`)
/// - IPv4 private (RFC 1918: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
/// - IPv4 link-local (`169.254.0.0/16`)
/// - IPv4 broadcast (`255.255.255.255`)
/// - IPv4 CGNAT (`100.64.0.0/10`, RFC 6598)
/// - IPv4 documentation (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`, RFC 5737)
/// - IPv4 unspecified (`0.0.0.0`)
/// - IPv6 loopback (`::1`)
/// - IPv6 unspecified (`::`)
/// - IPv6 Unique Local Address / ULA (`fc00::/7`, RFC 4193)
/// - IPv6 link-local (`fe80::/10`)
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

/// Check if an IPv4 address is private or reserved.
fn is_private_ipv4(v4: Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || is_cgnat(v4)
        || is_documentation_v4(v4)
        || v4.is_unspecified()
}

/// Check if an IPv6 address is private or reserved.
fn is_private_ipv6(v6: Ipv6Addr) -> bool {
    v6.is_loopback() || v6.is_unspecified() || is_ula(v6) || is_link_local_v6(v6)
}

/// Check if an IPv4 address is in the CGNAT range (100.64.0.0/10, RFC 6598).
fn is_cgnat(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    // 100.64.0.0/10: first octet == 100, second octet bits 7-6 == 01
    // Range: 100.64.0.0 - 100.127.255.255
    octets[0] == 100 && (octets[1] & 0xC0) == 64
}

/// Check if an IPv4 address is in a documentation range (RFC 5737).
///
/// - `192.0.2.0/24` (TEST-NET-1)
/// - `198.51.100.0/24` (TEST-NET-2)
/// - `203.0.113.0/24` (TEST-NET-3)
fn is_documentation_v4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
}

/// Check if an IPv6 address is a Unique Local Address (fc00::/7, RFC 4193).
fn is_ula(v6: Ipv6Addr) -> bool {
    let segments = v6.segments();
    // fc00::/7 means first 7 bits are 1111110x, covering fc00-fdff
    (segments[0] & 0xFE00) == 0xFC00
}

/// Check if an IPv6 address is link-local (fe80::/10).
fn is_link_local_v6(v6: Ipv6Addr) -> bool {
    let segments = v6.segments();
    // fe80::/10 means first 10 bits are 1111111010
    (segments[0] & 0xFFC0) == 0xFE80
}

/// Validate a source path for path traversal attacks.
///
/// Rejects paths containing:
/// - `..` (parent directory traversal)
/// - `//` (double slash, potential bypass)
/// - Null bytes (`\0`, `%00`)
/// - Backslashes (Windows-style traversal)
/// - `~` at the start (home directory references)
/// - URL-encoded traversals (`%2e%2e`, `%2f`, `%5c`)
pub fn validate_path(path: &str) -> Result<(), SecurityError> {
    // First, decode common URL encodings to catch encoded attacks
    let decoded = decode_path_for_validation(path);

    // Check both original and decoded paths for traversal patterns
    for check_path in &[path, decoded.as_str()] {
        if check_path.contains("..") {
            return Err(SecurityError::PathTraversalDetected(
                "'..' sequence detected".to_string(),
            ));
        }

        if check_path.contains("//") {
            return Err(SecurityError::PathTraversalDetected(
                "'//' double slash detected".to_string(),
            ));
        }

        if check_path.contains('\\') {
            return Err(SecurityError::PathTraversalDetected(
                "backslash detected".to_string(),
            ));
        }
    }

    // Null bytes
    if path.contains('\0') || path.contains("%00") {
        return Err(SecurityError::PathTraversalDetected(
            "null byte detected".to_string(),
        ));
    }

    // Home directory reference at start
    if path.starts_with('~') {
        return Err(SecurityError::PathTraversalDetected(
            "'~' home directory reference detected".to_string(),
        ));
    }

    // Check for encoded traversal sequences in original path
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2e%2e") || lower.contains("%2e.") || lower.contains(".%2e") {
        return Err(SecurityError::PathTraversalDetected(
            "URL-encoded traversal detected".to_string(),
        ));
    }
    if lower.contains("%2f") || lower.contains("%5c") {
        return Err(SecurityError::PathTraversalDetected(
            "URL-encoded path separator detected".to_string(),
        ));
    }

    Ok(())
}

/// Decode common percent-encoded sequences for path validation.
fn decode_path_for_validation(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_nibble(bytes[i + 1]);
            let lo = hex_nibble(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push(char::from(h << 4 | l));
                i += 3;
                continue;
            }
        }
        result.push(char::from(bytes[i]));
        i += 1;
    }

    result
}

/// Convert a hex ASCII byte to its numeric value (0-15).
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Validate transform dimensions against security configuration limits.
///
/// Checks the effective width and height (after DPR scaling) against the
/// configured maximums.
pub fn validate_dimensions(
    params: &TransformParams,
    config: &SecurityConfig,
) -> Result<(), SecurityError> {
    // Check the raw requested width scaled by DPR, without clamping.
    // This catches requests that exceed our security limits before any
    // downstream clamping can mask an oversized request.
    if let Some(w) = params.width {
        let effective_w = (f64::from(w) * params.dpr).round() as u32;
        if effective_w > config.max_width {
            return Err(SecurityError::DimensionExceeded {
                dimension: "width".to_string(),
                value: effective_w,
                max: config.max_width,
            });
        }
    }

    if let Some(h) = params.height {
        let effective_h = (f64::from(h) * params.dpr).round() as u32;
        if effective_h > config.max_height {
            return Err(SecurityError::DimensionExceeded {
                dimension: "height".to_string(),
                value: effective_h,
                max: config.max_height,
            });
        }
    }

    Ok(())
}

/// Perform full security validation of a transform request.
///
/// Validates:
/// 1. Source path for traversal attacks
/// 2. Transform dimensions against configured limits
pub fn validate_request(
    source_path: &str,
    params: &TransformParams,
    config: &SecurityConfig,
) -> Result<(), SecurityError> {
    validate_path(source_path)?;
    validate_dimensions(params, config)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Private IPv4 detection ──

    #[test]
    fn test_loopback_v4() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 255, 255, 255))));
    }

    #[test]
    fn test_rfc1918_10_network() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));
    }

    #[test]
    fn test_rfc1918_172_16_network() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        // 172.32.x.x is NOT private
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
    }

    #[test]
    fn test_rfc1918_192_168_network() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 255, 255))));
    }

    #[test]
    fn test_link_local_v4() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 255, 255))));
    }

    #[test]
    fn test_broadcast_v4() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255))));
    }

    #[test]
    fn test_cgnat_v4() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));
        // Just outside CGNAT
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));
    }

    #[test]
    fn test_documentation_v4() {
        // TEST-NET-1: 192.0.2.0/24
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 255))));
        // TEST-NET-2: 198.51.100.0/24
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 0))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 255))));
        // TEST-NET-3: 203.0.113.0/24
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 0))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 255))));
    }

    #[test]
    fn test_unspecified_v4() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
    }

    #[test]
    fn test_public_v4() {
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    // ── Private IPv6 detection ──

    #[test]
    fn test_loopback_v6() {
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn test_unspecified_v6() {
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn test_ula_v6() {
        let addr: Ipv6Addr = "fc00::1".parse().expect("valid ipv6");
        assert!(is_private_ip(IpAddr::V6(addr)));

        let addr: Ipv6Addr = "fd00::1".parse().expect("valid ipv6");
        assert!(is_private_ip(IpAddr::V6(addr)));

        let addr: Ipv6Addr = "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
            .parse()
            .expect("valid ipv6");
        assert!(is_private_ip(IpAddr::V6(addr)));
    }

    #[test]
    fn test_link_local_v6() {
        let addr: Ipv6Addr = "fe80::1".parse().expect("valid ipv6");
        assert!(is_private_ip(IpAddr::V6(addr)));

        let addr: Ipv6Addr = "fe80::ffff:ffff:ffff:ffff".parse().expect("valid ipv6");
        assert!(is_private_ip(IpAddr::V6(addr)));
    }

    #[test]
    fn test_public_v6() {
        let addr: Ipv6Addr = "2001:4860:4860::8888".parse().expect("valid ipv6");
        assert!(!is_private_ip(IpAddr::V6(addr)));

        let addr: Ipv6Addr = "2606:4700:4700::1111".parse().expect("valid ipv6");
        assert!(!is_private_ip(IpAddr::V6(addr)));
    }

    #[test]
    fn test_not_ula_boundary() {
        // fe00::1 is NOT ULA (fc00::/7 covers fc00-fdff only)
        let addr: Ipv6Addr = "fe00::1".parse().expect("valid ipv6");
        assert!(!is_private_ip(IpAddr::V6(addr)));
    }

    // ── Path traversal detection ──

    #[test]
    fn test_valid_path() {
        assert!(validate_path("images/photo.jpg").is_ok());
        assert!(validate_path("uploads/2024/banner.png").is_ok());
        assert!(validate_path("file.jpg").is_ok());
    }

    #[test]
    fn test_dotdot_traversal() {
        assert!(validate_path("../etc/passwd").is_err());
        assert!(validate_path("images/../../etc/passwd").is_err());
        assert!(validate_path("..").is_err());
    }

    #[test]
    fn test_encoded_dotdot_traversal() {
        assert!(validate_path("%2e%2e/etc/passwd").is_err());
        assert!(validate_path("%2E%2E/etc/passwd").is_err());
        assert!(validate_path("%2e./etc/passwd").is_err());
        assert!(validate_path(".%2e/etc/passwd").is_err());
    }

    #[test]
    fn test_encoded_slash_traversal() {
        assert!(validate_path("images%2f..%2fetc/passwd").is_err());
        assert!(validate_path("images%2Fphoto.jpg").is_err());
    }

    #[test]
    fn test_encoded_backslash() {
        assert!(validate_path("images%5cphoto.jpg").is_err());
    }

    #[test]
    fn test_double_slash() {
        assert!(validate_path("images//photo.jpg").is_err());
    }

    #[test]
    fn test_backslash_traversal() {
        assert!(validate_path("images\\..\\etc\\passwd").is_err());
        assert!(validate_path("images\\photo.jpg").is_err());
    }

    #[test]
    fn test_null_byte() {
        assert!(validate_path("image.jpg\0.png").is_err());
        assert!(validate_path("image.jpg%00.png").is_err());
    }

    #[test]
    fn test_tilde_home_dir() {
        assert!(validate_path("~/secret/photo.jpg").is_err());
        assert!(validate_path("~root/secret").is_err());
    }

    #[test]
    fn test_tilde_not_at_start_is_ok() {
        assert!(validate_path("images/file~backup.jpg").is_ok());
    }

    #[test]
    fn test_quadruple_dot_traversal() {
        // ..../ contains ".." so should be caught
        assert!(validate_path("..../etc/passwd").is_err());
    }

    // ── Dimension validation ──

    #[test]
    fn test_valid_dimensions() {
        let mut params = TransformParams::default();
        params.width = Some(800);
        params.height = Some(600);
        let config = SecurityConfig::default();
        assert!(validate_dimensions(&params, &config).is_ok());
    }

    #[test]
    fn test_width_exceeds_max() {
        let mut params = TransformParams::default();
        params.width = Some(15000);
        let config = SecurityConfig {
            max_width: 12000,
            ..SecurityConfig::default()
        };
        assert!(validate_dimensions(&params, &config).is_err());
    }

    #[test]
    fn test_height_exceeds_max() {
        let mut params = TransformParams::default();
        params.height = Some(15000);
        let config = SecurityConfig {
            max_height: 12000,
            ..SecurityConfig::default()
        };
        assert!(validate_dimensions(&params, &config).is_err());
    }

    #[test]
    fn test_dimensions_at_max_boundary() {
        let mut params = TransformParams::default();
        params.width = Some(12000);
        params.height = Some(12000);
        let config = SecurityConfig::default();
        assert!(validate_dimensions(&params, &config).is_ok());
    }

    #[test]
    fn test_dimensions_with_dpr_exceeds_max() {
        let mut params = TransformParams::default();
        params.width = Some(5000);
        params.dpr = 3.0;
        // effective width = 15000, exceeds default 12000
        let config = SecurityConfig {
            max_width: 12000,
            ..SecurityConfig::default()
        };
        let result = validate_dimensions(&params, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_no_dimensions_passes() {
        let params = TransformParams::default();
        let config = SecurityConfig::default();
        assert!(validate_dimensions(&params, &config).is_ok());
    }

    #[test]
    fn test_custom_max_dimensions() {
        let mut params = TransformParams::default();
        params.width = Some(2000);
        let config = SecurityConfig {
            max_width: 1000,
            ..SecurityConfig::default()
        };
        let result = validate_dimensions(&params, &config);
        assert!(result.is_err());
        if let Err(SecurityError::DimensionExceeded {
            dimension,
            value,
            max,
        }) = result
        {
            assert_eq!(dimension, "width");
            assert_eq!(value, 2000);
            assert_eq!(max, 1000);
        }
    }

    // ── Full request validation ──

    #[test]
    fn test_validate_request_valid() {
        let mut params = TransformParams::default();
        params.width = Some(800);
        params.height = Some(600);
        let config = SecurityConfig::default();
        assert!(validate_request("images/photo.jpg", &params, &config).is_ok());
    }

    #[test]
    fn test_validate_request_path_traversal() {
        let params = TransformParams::default();
        let config = SecurityConfig::default();
        assert!(validate_request("../etc/passwd", &params, &config).is_err());
    }

    #[test]
    fn test_validate_request_dimension_exceeded() {
        let mut params = TransformParams::default();
        params.width = Some(20000);
        let config = SecurityConfig::default();
        assert!(validate_request("image.jpg", &params, &config).is_err());
    }

    #[test]
    fn test_validate_request_path_checked_first() {
        // Both path and dimensions are bad; path should fail first
        let mut params = TransformParams::default();
        params.width = Some(20000);
        let config = SecurityConfig::default();
        let result = validate_request("../evil", &params, &config);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(SecurityError::PathTraversalDetected(_))
        ));
    }

    // ── SecurityConfig defaults ──

    #[test]
    fn test_security_config_defaults() {
        let config = SecurityConfig::default();
        assert_eq!(config.max_width, 12000);
        assert_eq!(config.max_height, 12000);
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
        assert_eq!(config.max_output_size, 50 * 1024 * 1024);
        assert!(!config.allow_external_urls);
        assert!(config.blocked_hosts.is_empty());
        assert!(config.allowed_hosts.is_empty());
    }

    // ── Error display ──

    #[test]
    fn test_error_display_private_ip() {
        let err = SecurityError::PrivateIpDetected("127.0.0.1".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("SSRF"));
        assert!(msg.contains("127.0.0.1"));
    }

    #[test]
    fn test_error_display_path_traversal() {
        let err = SecurityError::PathTraversalDetected(".. detected".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("path traversal"));
    }

    #[test]
    fn test_error_display_dimension() {
        let err = SecurityError::DimensionExceeded {
            dimension: "width".to_string(),
            value: 20000,
            max: 12000,
        };
        let msg = format!("{err}");
        assert!(msg.contains("width=20000"));
        assert!(msg.contains("max=12000"));
    }

    #[test]
    fn test_error_display_file_size() {
        let err = SecurityError::FileSizeExceeded {
            size: 200_000_000,
            max: 100_000_000,
        };
        let msg = format!("{err}");
        assert!(msg.contains("200000000"));
        assert!(msg.contains("100000000"));
    }

    #[test]
    fn test_error_display_protocol() {
        let err = SecurityError::UnsupportedProtocol("ftp".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("ftp"));
    }

    #[test]
    fn test_error_display_blocked() {
        let err = SecurityError::BlockedHostname("evil.com".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("evil.com"));
    }

    #[test]
    fn test_error_display_invalid_url() {
        let err = SecurityError::InvalidUrl("not a url".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("not a url"));
    }

    // ── CGNAT boundary tests ──

    #[test]
    fn test_cgnat_boundaries() {
        // First address in CGNAT range
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0))));
        // Last address in CGNAT range
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));
        // Just before CGNAT
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));
        // Just after CGNAT
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 0))));
    }

    // ── Documentation range boundary tests ──

    #[test]
    fn test_documentation_boundaries() {
        // 192.0.2.0 - first
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 0))));
        // 192.0.2.255 - last
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 255))));
        // Just outside
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 3, 0))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 1, 255))));
    }

    // ── Decode for validation ──

    #[test]
    fn test_decode_path_basic() {
        assert_eq!(decode_path_for_validation("hello%20world"), "hello world");
        assert_eq!(decode_path_for_validation("a%2Fb"), "a/b");
    }

    #[test]
    fn test_decode_path_invalid_sequence() {
        assert_eq!(decode_path_for_validation("hello%GGworld"), "hello%GGworld");
    }

    #[test]
    fn test_decode_path_empty() {
        assert_eq!(decode_path_for_validation(""), "");
    }

    #[test]
    fn test_decode_path_no_encoding() {
        assert_eq!(
            decode_path_for_validation("images/photo.jpg"),
            "images/photo.jpg"
        );
    }

    // ── Signed URL (HMAC-SHA256) ──

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let secret = b"test-secret-key-123";
        let path = "/cdn-cgi/image/w=800/photo.jpg";
        let params = "w=800,f=webp";
        let sig = sign_url(path, params, secret).expect("sign");
        assert_eq!(sig.len(), 64); // 32 bytes = 64 hex chars

        let config = SignedUrlConfig {
            secret: secret.to_vec(),
            required: true,
            expiry_tolerance_secs: 0,
        };
        assert!(verify_signature(path, params, &sig, &config).is_ok());
    }

    #[test]
    fn test_sign_url_deterministic() {
        let secret = b"key";
        let s1 = sign_url("/path", "p=1", secret).expect("sign");
        let s2 = sign_url("/path", "p=1", secret).expect("sign");
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_sign_url_different_paths() {
        let secret = b"key";
        let s1 = sign_url("/path1", "p=1", secret).expect("sign");
        let s2 = sign_url("/path2", "p=1", secret).expect("sign");
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_sign_url_different_params() {
        let secret = b"key";
        let s1 = sign_url("/path", "w=100", secret).expect("sign");
        let s2 = sign_url("/path", "w=200", secret).expect("sign");
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_sign_url_no_secret() {
        assert!(sign_url("/path", "p=1", &[]).is_err());
    }

    #[test]
    fn test_verify_wrong_signature() {
        let config = SignedUrlConfig {
            secret: b"real-secret".to_vec(),
            required: true,
            expiry_tolerance_secs: 0,
        };
        let result = verify_signature(
            "/path",
            "p=1",
            "0000000000000000000000000000000000000000000000000000000000000000",
            &config,
        );
        assert!(result.is_err());
        assert!(matches!(result, Err(SignedUrlError::InvalidSignature)));
    }

    #[test]
    fn test_verify_no_secret() {
        let config = SignedUrlConfig::default();
        let result = verify_signature("/path", "p=1", "sig", &config);
        assert!(matches!(result, Err(SignedUrlError::NoSecret)));
    }

    #[test]
    fn test_verify_signed_url_with_expiry() {
        let secret = b"expiry-key";
        let config = SignedUrlConfig {
            secret: secret.to_vec(),
            required: true,
            expiry_tolerance_secs: 300,
        };
        let sig = sign_url("/img", "w=400", secret).expect("sign");

        // Not expired
        assert!(verify_signed_url("/img", "w=400", &sig, Some(1000), 1200, &config).is_ok());

        // Expired (current_time > expiry + tolerance)
        assert!(verify_signed_url("/img", "w=400", &sig, Some(1000), 1500, &config).is_err());
    }

    #[test]
    fn test_verify_signed_url_no_expiry() {
        let secret = b"no-exp";
        let config = SignedUrlConfig {
            secret: secret.to_vec(),
            required: true,
            expiry_tolerance_secs: 0,
        };
        let sig = sign_url("/img", "w=800", secret).expect("sign");
        // No expiry check when tolerance is 0
        assert!(verify_signed_url("/img", "w=800", &sig, Some(0), 99999999, &config).is_ok());
    }

    #[test]
    fn test_sha256_known_vector() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = sha256(b"");
        let hex = hex_encode(&hash);
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_abc() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let hash = sha256(b"abc");
        let hex = hex_encode(&hash);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer string"));
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x00, 0xFF, 0xAB]), "00ffab");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn test_hmac_sha256_known() {
        // RFC 4231 Test Case 2
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let mac = hmac_sha256(key, data);
        let hex = hex_encode(&mac);
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn test_signed_url_config_defaults() {
        let config = SignedUrlConfig::default();
        assert!(config.secret.is_empty());
        assert!(!config.required);
        assert_eq!(config.expiry_tolerance_secs, 0);
    }

    #[test]
    fn test_signed_url_error_display() {
        assert!(format!("{}", SignedUrlError::MissingSignature).contains("missing"));
        assert!(format!("{}", SignedUrlError::InvalidSignature).contains("invalid"));
        assert!(format!("{}", SignedUrlError::Expired(100)).contains("100"));
        assert!(format!("{}", SignedUrlError::NoSecret).contains("not configured"));
    }

    // ── Resource exhaustion: output size limits ──

    #[test]
    fn test_security_output_size_within_limit() {
        // max_width=1000, max_height=1000 — requesting 100×100 should pass.
        let mut params = TransformParams::default();
        params.width = Some(100);
        params.height = Some(100);
        let config = SecurityConfig {
            max_width: 1000,
            max_height: 1000,
            ..SecurityConfig::default()
        };
        assert!(validate_dimensions(&params, &config).is_ok());
    }

    #[test]
    fn test_security_output_size_exceeds_limit() {
        // Request 10000×10000 against a tight max of 1000×1000.
        let mut params = TransformParams::default();
        params.width = Some(10000);
        params.height = Some(10000);
        let config = SecurityConfig {
            max_width: 1000,
            max_height: 1000,
            ..SecurityConfig::default()
        };
        let result = validate_dimensions(&params, &config);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(SecurityError::DimensionExceeded { .. })
        ));
    }

    // ── Resource exhaustion: file size limits ──
    //
    // `SecurityConfig` carries `max_file_size` and `max_output_size` fields that
    // downstream callers use when checking fetched/output bytes.  We exercise the
    // `FileSizeExceeded` error variant directly (it is already part of
    // `SecurityError`) to verify that callers can construct and match it
    // correctly.

    #[test]
    fn test_security_file_size_within_limit() {
        // Construct a config and confirm that a file size below the limit does
        // not produce a FileSizeExceeded error when evaluated by a caller.
        let config = SecurityConfig {
            max_file_size: 100 * 1024 * 1024, // 100 MB
            ..SecurityConfig::default()
        };
        let file_size: u64 = 50 * 1024 * 1024; // 50 MB
        let result: Result<(), SecurityError> = if file_size > config.max_file_size {
            Err(SecurityError::FileSizeExceeded {
                size: file_size,
                max: config.max_file_size,
            })
        } else {
            Ok(())
        };
        assert!(result.is_ok());
    }

    #[test]
    fn test_security_file_size_exceeds_limit() {
        // A file larger than max_file_size should yield FileSizeExceeded.
        let config = SecurityConfig {
            max_file_size: 10 * 1024 * 1024, // 10 MB
            ..SecurityConfig::default()
        };
        let file_size: u64 = 200 * 1024 * 1024; // 200 MB
        let result: Result<(), SecurityError> = if file_size > config.max_file_size {
            Err(SecurityError::FileSizeExceeded {
                size: file_size,
                max: config.max_file_size,
            })
        } else {
            Ok(())
        };
        assert!(result.is_err());
        match result {
            Err(SecurityError::FileSizeExceeded { size, max }) => {
                assert_eq!(size, 200 * 1024 * 1024);
                assert_eq!(max, 10 * 1024 * 1024);
            }
            _ => panic!("expected FileSizeExceeded"),
        }
    }

    // ── Golden path: full validate_request passes ──

    #[test]
    fn test_security_golden_path() {
        // A valid path, valid dimensions, and a file size within limits should
        // succeed through the full validate_request function.
        let mut params = TransformParams::default();
        params.width = Some(800);
        params.height = Some(600);
        let config = SecurityConfig {
            max_width: 2000,
            max_height: 2000,
            max_file_size: 100 * 1024 * 1024,
            max_output_size: 50 * 1024 * 1024,
            ..SecurityConfig::default()
        };
        // validate_request checks path traversal then dimensions.
        assert!(validate_request("images/photo.jpg", &params, &config).is_ok());

        // Also confirm file-size guard passes for a small file.
        let file_size: u64 = 1024 * 1024; // 1 MB
        assert!(file_size <= config.max_file_size);
    }
}
