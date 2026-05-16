//! Minimal `rtsp://` URL parser.
//!
//! We don't depend on a general URL crate here because RTSP URLs have a few
//! quirks (no fragment, path is opaque to the protocol) and we need to be
//! able to strip credentials and resolve `a=control:` relative URIs.

use crate::error::NetError;

/// Parsed RTSP URL.
#[derive(Debug, Clone)]
pub struct RtspUrl {
    /// Scheme; always `rtsp` for now.
    pub scheme: String,
    /// Optional `user:pass` extracted from the userinfo section.
    pub userinfo: Option<(String, String)>,
    /// Host portion (no port).
    pub host: String,
    /// Port; defaults to 554.
    pub port: u16,
    /// Path including any leading `/` (may be empty).
    pub path: String,
    /// Query string after `?` (without the `?`), or empty.
    pub query: String,
}

impl RtspUrl {
    /// Parse a full `rtsp://[user[:pass]@]host[:port]/path[?query]`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InvalidUrl`] if the scheme is not `rtsp://`,
    /// if an IPv6 literal is missing its closing `]`, or if the port
    /// is not a valid `u16`.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::RtspUrl;
    ///
    /// let u = RtspUrl::parse("rtsp://admin:secret@cam.local:8554/live").unwrap();
    /// assert_eq!(u.host, "cam.local");
    /// assert_eq!(u.port, 8554);
    /// assert_eq!(
    ///     u.userinfo,
    ///     Some(("admin".to_string(), "secret".to_string()))
    /// );
    /// ```
    pub fn parse(input: &str) -> Result<Self, NetError> {
        let rest = input.strip_prefix("rtsp://").ok_or_else(|| {
            NetError::InvalidUrl(format!("expected rtsp:// scheme, got {input:?}"))
        })?;

        let (authority, path_query) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        };

        let (userinfo, host_port) = match authority.rfind('@') {
            Some(idx) => {
                let (u, _) = authority.split_at(idx);
                let creds = match u.split_once(':') {
                    Some((user, pass)) => (user.to_string(), pass.to_string()),
                    None => (u.to_string(), String::new()),
                };
                (Some(creds), &authority[idx + 1..])
            }
            None => (None, authority),
        };

        let (host, port) = if let Some(stripped) = host_port.strip_prefix('[') {
            // IPv6 literal: `[2001:db8::1]:554`
            let end = stripped
                .find(']')
                .ok_or_else(|| NetError::InvalidUrl("missing ] in IPv6 literal".into()))?;
            let host = stripped[..end].to_string();
            let port = match stripped[end + 1..].strip_prefix(':') {
                Some(p) => p
                    .parse::<u16>()
                    .map_err(|e| NetError::InvalidUrl(format!("bad port: {e}")))?,
                None => 554,
            };
            (host, port)
        } else {
            match host_port.rsplit_once(':') {
                Some((h, p)) => (
                    h.to_string(),
                    p.parse::<u16>()
                        .map_err(|e| NetError::InvalidUrl(format!("bad port: {e}")))?,
                ),
                None => (host_port.to_string(), 554),
            }
        };

        let (path, query) = match path_query.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (path_query.to_string(), String::new()),
        };

        Ok(Self {
            scheme: "rtsp".into(),
            userinfo,
            host,
            port,
            path,
            query,
        })
    }

    /// URL with userinfo stripped, suitable for the request-URI on the wire.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::RtspUrl;
    ///
    /// let u = RtspUrl::parse("rtsp://admin:secret@cam/live").unwrap();
    /// // Credentials must not leak onto the wire.
    /// assert_eq!(u.request_uri(), "rtsp://cam/live");
    /// ```
    #[must_use]
    pub fn request_uri(&self) -> String {
        let mut out = format!("rtsp://{}", self.host);
        if self.port != 554 {
            out.push_str(&format!(":{}", self.port));
        }
        if self.path.is_empty() {
            out.push('/');
        } else {
            out.push_str(&self.path);
        }
        if !self.query.is_empty() {
            out.push('?');
            out.push_str(&self.query);
        }
        out
    }

    /// Authority portion `host:port` for `connect()`.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::RtspUrl;
    /// let u = RtspUrl::parse("rtsp://10.0.0.5:8554/s").unwrap();
    /// assert_eq!(u.authority(), "10.0.0.5:8554");
    /// ```
    #[must_use]
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Resolve an `a=control:` value against this URL.
    ///
    /// Rules per RFC 2326 §C.1.1:
    /// - `*` → use the base URL as-is.
    /// - absolute `rtsp://` → use as-is.
    /// - anything else → append to the base, using `/` between unless the
    ///   base already ends in one.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::RtspUrl;
    ///
    /// let u = RtspUrl::parse("rtsp://cam/live").unwrap();
    /// assert_eq!(u.resolve_control("trackID=1"), "rtsp://cam/live/trackID=1");
    /// assert_eq!(u.resolve_control("*"), "rtsp://cam/live");
    /// assert_eq!(
    ///     u.resolve_control("rtsp://other/t"),
    ///     "rtsp://other/t"
    /// );
    /// ```
    #[must_use]
    pub fn resolve_control(&self, control: &str) -> String {
        if control == "*" {
            return self.request_uri();
        }
        if control.starts_with("rtsp://") || control.starts_with("rtsps://") {
            return control.to_string();
        }
        let base = self.request_uri();
        if base.ends_with('/') || control.starts_with('/') {
            format!("{base}{control}")
        } else {
            format!("{base}/{control}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_url() {
        let u = RtspUrl::parse("rtsp://camera.local/live").unwrap();
        assert_eq!(u.host, "camera.local");
        assert_eq!(u.port, 554);
        assert_eq!(u.path, "/live");
        assert!(u.userinfo.is_none());
    }

    #[test]
    fn parses_url_with_port() {
        let u = RtspUrl::parse("rtsp://10.0.0.5:8554/stream1").unwrap();
        assert_eq!(u.port, 8554);
    }

    #[test]
    fn parses_url_with_credentials() {
        let u = RtspUrl::parse("rtsp://admin:hunter2@camera/live").unwrap();
        assert_eq!(
            u.userinfo,
            Some(("admin".to_string(), "hunter2".to_string()))
        );
        // request_uri must NOT leak credentials onto the wire.
        assert_eq!(u.request_uri(), "rtsp://camera/live");
    }

    #[test]
    fn parses_query_string() {
        let u = RtspUrl::parse("rtsp://h/path?token=abc").unwrap();
        assert_eq!(u.path, "/path");
        assert_eq!(u.query, "token=abc");
        assert_eq!(u.request_uri(), "rtsp://h/path?token=abc");
    }

    #[test]
    fn parses_ipv6_literal() {
        let u = RtspUrl::parse("rtsp://[2001:db8::1]:554/s").unwrap();
        assert_eq!(u.host, "2001:db8::1");
        assert_eq!(u.port, 554);
    }

    #[test]
    fn resolves_absolute_control() {
        let u = RtspUrl::parse("rtsp://c/path").unwrap();
        assert_eq!(
            u.resolve_control("rtsp://other/track1"),
            "rtsp://other/track1"
        );
    }

    #[test]
    fn resolves_star_control() {
        let u = RtspUrl::parse("rtsp://c/path").unwrap();
        assert_eq!(u.resolve_control("*"), "rtsp://c/path");
    }

    #[test]
    fn resolves_relative_control() {
        let u = RtspUrl::parse("rtsp://c/path").unwrap();
        assert_eq!(u.resolve_control("trackID=1"), "rtsp://c/path/trackID=1");
    }

    #[test]
    fn rejects_non_rtsp_scheme() {
        assert!(RtspUrl::parse("http://x/y").is_err());
    }
}
