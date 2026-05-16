//! RTSP authentication.
//!
//! IP cameras overwhelmingly use HTTP Digest (RFC 2617) — Basic is also
//! accepted by older firmware but exposes the password in base64. This
//! module covers both, since real-world clients have to fall through
//! Basic on devices that lack Digest support.

use base64::Engine;

use crate::error::NetError;

/// Parsed `WWW-Authenticate:` challenge from a 401 response.
#[derive(Debug, Clone)]
pub enum Challenge {
    /// `Basic realm="<realm>"`.
    Basic {
        /// Authentication realm sent by the server.
        realm: String,
    },
    /// `Digest realm="...", nonce="...", algorithm=MD5, qop="auth"`.
    Digest {
        /// Authentication realm.
        realm: String,
        /// Server-supplied nonce.
        nonce: String,
        /// Optional opaque token to echo back.
        opaque: Option<String>,
        /// `qop` (quality-of-protection) list, e.g. `Some("auth")`.
        qop: Option<String>,
        /// Hash algorithm. We only support MD5 / unspecified.
        algorithm: Option<String>,
    },
}

/// Credentials supplied by the user.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Username, possibly from the `rtsp://user:pass@host/...` URL.
    pub username: String,
    /// Password.
    pub password: String,
}

impl Challenge {
    /// Parse a single `WWW-Authenticate` header value.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Protocol`] if the header doesn't start with a
    /// supported scheme name (`Basic` or `Digest`) or if a Digest
    /// challenge is missing the mandatory `realm` / `nonce` parameters.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::Challenge;
    /// let c = Challenge::parse(
    ///     "Digest realm=\"cam\", nonce=\"abc123\", algorithm=MD5, qop=\"auth\"",
    /// )
    /// .unwrap();
    /// assert!(matches!(c, Challenge::Digest { .. }));
    /// ```
    pub fn parse(header_value: &str) -> Result<Self, NetError> {
        let trimmed = header_value.trim_start();
        let (scheme, params) = trimmed
            .split_once(char::is_whitespace)
            .ok_or_else(|| NetError::Protocol(format!("malformed challenge: {trimmed:?}")))?;
        let params = parse_auth_params(params);
        match scheme.to_ascii_lowercase().as_str() {
            "basic" => {
                let realm = params
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("realm"))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                Ok(Self::Basic { realm })
            }
            "digest" => {
                let mut realm = None;
                let mut nonce = None;
                let mut opaque = None;
                let mut qop = None;
                let mut algorithm = None;
                for (k, v) in &params {
                    match k.to_ascii_lowercase().as_str() {
                        "realm" => realm = Some(v.clone()),
                        "nonce" => nonce = Some(v.clone()),
                        "opaque" => opaque = Some(v.clone()),
                        "qop" => qop = Some(v.clone()),
                        "algorithm" => algorithm = Some(v.clone()),
                        _ => {}
                    }
                }
                Ok(Self::Digest {
                    realm: realm.ok_or_else(|| {
                        NetError::Protocol("Digest challenge missing realm".into())
                    })?,
                    nonce: nonce.ok_or_else(|| {
                        NetError::Protocol("Digest challenge missing nonce".into())
                    })?,
                    opaque,
                    qop,
                    algorithm,
                })
            }
            other => Err(NetError::Protocol(format!(
                "unsupported auth scheme: {other}"
            ))),
        }
    }

    /// Build the `Authorization:` header value for a single request.
    ///
    /// `method` is the RTSP method name (e.g. `"DESCRIBE"`).
    /// `uri` is the absolute request URI.
    /// `nc` is the nonce-count: `1` for the first request that uses this
    /// nonce, incrementing thereafter. `cnonce` is a client nonce — any
    /// random hex string; callers usually pass a UUID-derived value.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::{Challenge, Credentials};
    ///
    /// let c = Challenge::Basic { realm: "cam".into() };
    /// let creds = Credentials {
    ///     username: "Aladdin".into(),
    ///     password: "open sesame".into(),
    /// };
    /// let header = c.build_authorization(&creds, "DESCRIBE", "rtsp://x/y", 1, "abc");
    /// assert_eq!(header, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    /// ```
    pub fn build_authorization(
        &self,
        creds: &Credentials,
        method: &str,
        uri: &str,
        nc: u32,
        cnonce: &str,
    ) -> String {
        match self {
            Self::Basic { .. } => {
                let raw = format!("{}:{}", creds.username, creds.password);
                let b64 = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
                format!("Basic {b64}")
            }
            Self::Digest {
                realm,
                nonce,
                opaque,
                qop,
                algorithm: _,
            } => {
                let ha1 = md5_hex(&format!(
                    "{}:{}:{}",
                    creds.username, realm, creds.password
                ));
                let ha2 = md5_hex(&format!("{method}:{uri}"));

                let (response, qop_field) = match qop.as_deref() {
                    Some(qop_value) if qop_value.split(',').any(|q| q.trim() == "auth") => {
                        // Per RFC 2617 §3.2.2.1, the response with qop=auth is:
                        //   MD5( HA1 : nonce : nc : cnonce : qop : HA2 )
                        let nc_str = format!("{nc:08x}");
                        let r = md5_hex(&format!(
                            "{ha1}:{nonce}:{nc_str}:{cnonce}:auth:{ha2}"
                        ));
                        let field = format!(
                            ", qop=auth, nc={nc_str}, cnonce=\"{cnonce}\""
                        );
                        (r, field)
                    }
                    _ => {
                        // RFC 2069 fallback: MD5( HA1 : nonce : HA2 ).
                        let r = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
                        (r, String::new())
                    }
                };

                let mut auth = format!(
                    "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
                    creds.username, realm, nonce, uri, response
                );
                auth.push_str(&qop_field);
                if let Some(op) = opaque {
                    auth.push_str(&format!(", opaque=\"{op}\""));
                }
                auth
            }
        }
    }
}

/// Parse `key="value", key2=value2, key3="quoted, with comma"` into a vec.
///
/// Quoted-string values may contain commas; values without quotes terminate
/// at the next comma. Whitespace around `,` and `=` is ignored.
fn parse_auth_params(input: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace and commas.
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read key up to '='.
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' {
            i += 1;
        }
        let key = std::str::from_utf8(&bytes[key_start..i]).unwrap_or("").trim();
        if i >= bytes.len() {
            break;
        }
        i += 1; // skip '='

        // Read value; quoted or unquoted.
        if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let val_start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                // Allow escaped quotes (\\\") — uncommon in RTSP but legal HTTP.
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            let value = std::str::from_utf8(&bytes[val_start..i]).unwrap_or("").to_string();
            out.push((key.to_string(), value));
            if i < bytes.len() {
                i += 1; // skip closing quote
            }
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            let value = std::str::from_utf8(&bytes[val_start..i])
                .unwrap_or("")
                .trim()
                .to_string();
            out.push((key.to_string(), value));
        }
    }
    out
}

/// MD5 hex digest (lowercase) — used by HTTP Digest per RFC 2617.
///
/// We implement MD5 inline rather than pulling a dependency. The MD5 algorithm
/// is broken cryptographically but is the wire-level requirement for RFC 2617;
/// callers should not use this routine for anything else.
fn md5_hex(input: &str) -> String {
    let digest = md5(input.as_bytes());
    let mut out = String::with_capacity(32);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// MD5 (RFC 1321) — implementation kept self-contained.
fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let orig_len = input.len() as u64;
    let bit_len = orig_len.wrapping_mul(8);

    // Pad: append 0x80, then zeros to make len % 64 == 56, then 8-byte length LE.
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            m[i] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vectors() {
        // RFC 1321 test vectors.
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex("message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            md5_hex("abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
    }

    #[test]
    fn parse_basic_challenge() {
        let c = Challenge::parse("Basic realm=\"IP Camera\"").unwrap();
        match c {
            Challenge::Basic { realm } => assert_eq!(realm, "IP Camera"),
            _ => panic!("expected basic"),
        }
    }

    #[test]
    fn parse_digest_challenge() {
        let c = Challenge::parse(
            "Digest realm=\"4419b6353b21\", nonce=\"66a36d2a\", algorithm=MD5, qop=\"auth\"",
        )
        .unwrap();
        let Challenge::Digest {
            realm,
            nonce,
            qop,
            algorithm,
            ..
        } = c
        else {
            panic!("expected digest");
        };
        assert_eq!(realm, "4419b6353b21");
        assert_eq!(nonce, "66a36d2a");
        assert_eq!(qop.as_deref(), Some("auth"));
        assert_eq!(algorithm.as_deref(), Some("MD5"));
    }

    #[test]
    fn basic_authorization_round_trips_credentials() {
        let c = Challenge::Basic {
            realm: "x".into(),
        };
        let creds = Credentials {
            username: "Aladdin".into(),
            password: "open sesame".into(),
        };
        let header = c.build_authorization(&creds, "DESCRIBE", "rtsp://x/y", 1, "abc");
        // base64("Aladdin:open sesame") == "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        assert_eq!(header, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    }

    /// RFC 2617 §3.5 worked example values, adapted to RTSP DESCRIBE.
    ///
    /// We can't reuse the RFC's exact vector because the URI/method differ,
    /// but we cross-verify the same algorithm by computing the response by
    /// hand: HA1 = MD5("Mufasa:testrealm@host.com:Circle Of Life") and
    /// HA2 = MD5("DESCRIBE:rtsp://host/a"). Without qop, response =
    /// MD5(HA1:nonce:HA2).
    #[test]
    fn digest_without_qop_matches_manual_computation() {
        let c = Challenge::Digest {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            opaque: None,
            qop: None,
            algorithm: Some("MD5".into()),
        };
        let creds = Credentials {
            username: "Mufasa".into(),
            password: "Circle Of Life".into(),
        };
        let auth = c.build_authorization(&creds, "DESCRIBE", "rtsp://host/a", 1, "cnonce");
        let ha1 = md5_hex("Mufasa:testrealm@host.com:Circle Of Life");
        let ha2 = md5_hex("DESCRIBE:rtsp://host/a");
        let expected = md5_hex(&format!(
            "{ha1}:dcd98b7102dd2f0e8b11d0f600bfb0c093:{ha2}"
        ));
        assert!(
            auth.contains(&format!("response=\"{expected}\"")),
            "auth header was: {auth}"
        );
    }

    #[test]
    fn digest_with_qop_uses_nc_and_cnonce() {
        let c = Challenge::Digest {
            realm: "r".into(),
            nonce: "n".into(),
            opaque: Some("op".into()),
            qop: Some("auth".into()),
            algorithm: Some("MD5".into()),
        };
        let creds = Credentials {
            username: "u".into(),
            password: "p".into(),
        };
        let header = c.build_authorization(&creds, "PLAY", "rtsp://x/y", 7, "abc123");
        assert!(header.contains("qop=auth"));
        assert!(header.contains("nc=00000007"));
        assert!(header.contains("cnonce=\"abc123\""));
        assert!(header.contains("opaque=\"op\""));
    }
}
