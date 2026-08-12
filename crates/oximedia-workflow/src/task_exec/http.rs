//! Real HTTP execution for [`TaskType::HttpRequest`](crate::task::TaskType::HttpRequest).
//!
//! The request is sent with `reqwest`, which the workspace builds with the
//! `rustls-no-provider` feature to stay 100% Pure Rust. That means the
//! process must install a `rustls` [`CryptoProvider`] before the first TLS
//! handshake; [`ensure_crypto_provider`] does that once per process using
//! `rustls-rustcrypto` (same approach as `oximedia-storage`'s Azure client).
//!
//! # Configuration
//!
//! [`TaskType::HttpRequest`](crate::task::TaskType::HttpRequest) carries the
//! URL, method, headers and body. Two further knobs come from the enclosing
//! [`Task`](crate::task::Task), because the task-type schema is public API
//! shared with crates outside this one and cannot gain fields:
//!
//! - **Timeout** — [`Task::timeout`](crate::task::Task::timeout) is applied as
//!   the total request timeout.
//! - **Accepted statuses** — the `http.allow_status` key of
//!   [`Task::metadata`](crate::task::Task::metadata) overrides the default
//!   "2xx only" rule. See [`StatusAllowList::parse`].
//!
//! [`CryptoProvider`]: https://docs.rs/rustls/latest/rustls/crypto/struct.CryptoProvider.html

use std::collections::HashMap;
use std::sync::Once;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::error::{Result, WorkflowError};
use crate::task::HttpMethod;
use crate::task_exec::TaskOutcome;

/// Metadata key holding the accepted-status specification.
pub const ALLOW_STATUS_KEY: &str = "http.allow_status";

/// Maximum number of response-body characters retained in the task result.
const BODY_PREVIEW_CHARS: usize = 1024;

/// Process-wide guard ensuring the Pure-Rust `rustls` crypto provider is
/// installed at most once.
static INSTALL_CRYPTO_PROVIDER: Once = Once::new();

/// Installs the Pure-Rust `rustls-rustcrypto` crypto provider as the
/// process-wide default `rustls` `CryptoProvider`.
///
/// Idempotent: only the first call in a process installs anything, and a
/// provider installed earlier by another crate is left alone (that is not an
/// error for us, so the result is discarded).
pub fn ensure_crypto_provider() {
    INSTALL_CRYPTO_PROVIDER.call_once(|| {
        let _ = rustls_rustcrypto::provider().install_default();
    });
}

// ---------------------------------------------------------------------------
// Accepted-status specification
// ---------------------------------------------------------------------------

/// A single accepted-status pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusPattern {
    /// One exact status code (`404`).
    Exact(u16),
    /// An inclusive range of codes (`200-204`).
    Range(u16, u16),
}

impl StatusPattern {
    /// Returns `true` when `status` matches this pattern.
    const fn matches(self, status: u16) -> bool {
        match self {
            Self::Exact(code) => status == code,
            Self::Range(low, high) => status >= low && status <= high,
        }
    }
}

/// The set of HTTP status codes a task treats as success.
///
/// The default (no `http.allow_status` metadata key) is `200-299`: any other
/// status fails the task rather than being logged and ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusAllowList {
    patterns: Vec<StatusPattern>,
    /// The original specification text, for error messages.
    spec: String,
}

impl Default for StatusAllowList {
    fn default() -> Self {
        Self {
            patterns: vec![StatusPattern::Range(200, 299)],
            spec: "2xx".to_string(),
        }
    }
}

impl StatusAllowList {
    /// Parses an accepted-status specification.
    ///
    /// Accepted tokens, comma-separated and whitespace-insensitive:
    ///
    /// | Token | Meaning |
    /// |---|---|
    /// | `204` | exactly that status |
    /// | `2xx` | the whole class `200-299` (also `1xx`…`5xx`) |
    /// | `200-204` | an inclusive range |
    /// | `any` / `*` | every status (`100-599`) |
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::InvalidParameter`] when a token is not one of
    /// the forms above or contains an out-of-range code.
    pub fn parse(spec: &str) -> Result<Self> {
        let invalid = |detail: &str| WorkflowError::InvalidParameter {
            param: ALLOW_STATUS_KEY.to_string(),
            value: format!("{spec} ({detail}; accepted: `204`, `2xx`, `200-204`, `any`)"),
        };

        let mut patterns = Vec::new();
        for token in spec.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let lower = token.to_ascii_lowercase();

            if lower == "any" || lower == "*" {
                patterns.push(StatusPattern::Range(100, 599));
                continue;
            }

            if let Some(class) = lower.strip_suffix("xx") {
                let class: u16 = class
                    .parse()
                    .map_err(|_| invalid(&format!("`{token}` is not a status class")))?;
                if !(1..=5).contains(&class) {
                    return Err(invalid(&format!("status class `{token}` is not 1xx-5xx")));
                }
                patterns.push(StatusPattern::Range(class * 100, class * 100 + 99));
                continue;
            }

            if let Some((low, high)) = lower.split_once('-') {
                let low: u16 = low
                    .trim()
                    .parse()
                    .map_err(|_| invalid(&format!("`{token}` has a non-numeric range start")))?;
                let high: u16 = high
                    .trim()
                    .parse()
                    .map_err(|_| invalid(&format!("`{token}` has a non-numeric range end")))?;
                if low > high {
                    return Err(invalid(&format!("range `{token}` is inverted")));
                }
                if !(100..=599).contains(&low) || !(100..=599).contains(&high) {
                    return Err(invalid(&format!("range `{token}` is outside 100-599")));
                }
                patterns.push(StatusPattern::Range(low, high));
                continue;
            }

            let code: u16 = lower
                .parse()
                .map_err(|_| invalid(&format!("`{token}` is not a status code")))?;
            if !(100..=599).contains(&code) {
                return Err(invalid(&format!("status `{token}` is outside 100-599")));
            }
            patterns.push(StatusPattern::Exact(code));
        }

        if patterns.is_empty() {
            return Err(invalid("no status patterns given"));
        }

        Ok(Self {
            patterns,
            spec: spec.trim().to_string(),
        })
    }

    /// Reads the allow-list from task metadata, falling back to `2xx`.
    ///
    /// # Errors
    ///
    /// Returns an error when the `http.allow_status` value is malformed.
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Result<Self> {
        match metadata.get(ALLOW_STATUS_KEY) {
            Some(spec) => Self::parse(spec),
            None => Ok(Self::default()),
        }
    }

    /// Returns `true` when `status` is accepted.
    #[must_use]
    pub fn accepts(&self, status: u16) -> bool {
        self.patterns.iter().any(|p| p.matches(status))
    }

    /// The specification text this list was parsed from.
    #[must_use]
    pub fn spec(&self) -> &str {
        &self.spec
    }
}

// ---------------------------------------------------------------------------
// Request execution
// ---------------------------------------------------------------------------

/// Maps the workflow method enum onto a `reqwest` method.
fn reqwest_method(method: &HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Delete => reqwest::Method::DELETE,
        HttpMethod::Patch => reqwest::Method::PATCH,
    }
}

/// Truncates `text` to [`BODY_PREVIEW_CHARS`] characters (never splitting a
/// UTF-8 code point), appending an ellipsis marker when truncation occurred.
fn body_preview(text: &str) -> String {
    if text.chars().count() <= BODY_PREVIEW_CHARS {
        return text.to_string();
    }
    let mut preview: String = text.chars().take(BODY_PREVIEW_CHARS).collect();
    preview.push_str("…[truncated]");
    preview
}

/// Sends the HTTP request described by a
/// [`TaskType::HttpRequest`](crate::task::TaskType::HttpRequest) task.
///
/// The request honours `method`, `headers` and `body` from the task type and
/// `timeout` / `allow_status` from the enclosing task (see the module docs).
/// The returned [`TaskOutcome`] records the status code, the final URL after
/// redirects, the elapsed time, the response size and a bounded body preview.
///
/// # Errors
///
/// Returns an error when:
/// - the URL is not parseable,
/// - a header name or value cannot be encoded,
/// - the client cannot be built or the request cannot be sent (DNS, TLS,
///   connection refused, timeout),
/// - the response status is not accepted by `allow_status`.
pub async fn execute_http_request(
    url: &str,
    method: &HttpMethod,
    headers: &HashMap<String, String>,
    body: Option<&str>,
    timeout: Duration,
    allow_status: &StatusAllowList,
) -> Result<TaskOutcome> {
    ensure_crypto_provider();

    // Reject a malformed URL before building a client.
    let parsed = url::Url::parse(url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(WorkflowError::Http(format!(
            "unsupported URL scheme `{}` in {url}; only http and https are supported",
            parsed.scheme()
        )));
    }

    let method = reqwest_method(method);

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| WorkflowError::Http(format!("HTTP client construction failed: {e}")))?;

    let mut request = client.request(method.clone(), parsed.clone());
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.body(body.to_string());
    }

    debug!(
        "HTTP {} {} (timeout {:?}, accept {})",
        method,
        parsed,
        timeout,
        allow_status.spec()
    );

    let started = Instant::now();
    let response = request
        .send()
        .await
        .map_err(|e| WorkflowError::Http(format!("HTTP {method} {parsed} failed: {e}")))?;

    let status = response.status();
    let final_url = response.url().to_string();
    let text = response.text().await.map_err(|e| {
        WorkflowError::Http(format!("HTTP {method} {parsed}: body read failed: {e}"))
    })?;
    let elapsed = started.elapsed();

    let status_code = status.as_u16();
    if !allow_status.accepts(status_code) {
        return Err(WorkflowError::Http(format!(
            "HTTP {method} {parsed} returned {status_code} {}, which is not accepted by `{}`: {}",
            status.canonical_reason().unwrap_or("Unknown"),
            allow_status.spec(),
            body_preview(&text)
        )));
    }

    info!(
        "HTTP {} {} -> {} in {:?} ({} bytes)",
        method,
        final_url,
        status_code,
        elapsed,
        text.len()
    );

    Ok(TaskOutcome::with_data(serde_json::json!({
        "kind": "http_request",
        "method": method.as_str(),
        "url": final_url,
        "status": status_code,
        "status_text": status.canonical_reason().unwrap_or("Unknown"),
        "elapsed_ms": elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
        "response_bytes": text.len(),
        "body_preview": body_preview(&text),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_allow_list_accepts_only_2xx() {
        let list = StatusAllowList::default();
        assert!(list.accepts(200));
        assert!(list.accepts(299));
        assert!(!list.accepts(300));
        assert!(!list.accepts(404));
        assert!(!list.accepts(500));
    }

    #[test]
    fn parse_class_exact_and_range() {
        let list = StatusAllowList::parse("2xx, 404, 500-503").expect("valid spec");
        assert!(list.accepts(201));
        assert!(list.accepts(404));
        assert!(list.accepts(502));
        assert!(!list.accepts(403));
        assert!(!list.accepts(504));
    }

    #[test]
    fn parse_any_accepts_everything() {
        let list = StatusAllowList::parse("any").expect("valid spec");
        assert!(list.accepts(100));
        assert!(list.accepts(599));
    }

    #[test]
    fn parse_rejects_garbage() {
        for spec in ["", "abc", "6xx", "204-200", "99", "600"] {
            assert!(
                StatusAllowList::parse(spec).is_err(),
                "spec {spec:?} must be rejected"
            );
        }
    }

    #[test]
    fn from_metadata_uses_default_when_absent() {
        let meta = HashMap::new();
        let list = StatusAllowList::from_metadata(&meta).expect("default");
        assert_eq!(list, StatusAllowList::default());
    }

    #[test]
    fn from_metadata_reads_key() {
        let mut meta = HashMap::new();
        meta.insert(ALLOW_STATUS_KEY.to_string(), "404".to_string());
        let list = StatusAllowList::from_metadata(&meta).expect("parsed");
        assert!(list.accepts(404));
        assert!(!list.accepts(200));
    }

    #[test]
    fn body_preview_truncates() {
        let long = "x".repeat(BODY_PREVIEW_CHARS * 2);
        let preview = body_preview(&long);
        assert!(preview.ends_with("…[truncated]"));
        assert_eq!(preview.chars().count(), BODY_PREVIEW_CHARS + 12);
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let err = execute_http_request(
            "ftp://example.invalid/file",
            &HttpMethod::Get,
            &HashMap::new(),
            None,
            Duration::from_secs(1),
            &StatusAllowList::default(),
        )
        .await
        .expect_err("ftp:// must be rejected");
        assert!(err.to_string().contains("unsupported URL scheme"));
    }

    #[tokio::test]
    async fn rejects_malformed_url() {
        let err = execute_http_request(
            "not a url",
            &HttpMethod::Get,
            &HashMap::new(),
            None,
            Duration::from_secs(1),
            &StatusAllowList::default(),
        )
        .await
        .expect_err("malformed URL must be rejected");
        assert!(matches!(err, WorkflowError::InvalidUrl(_)));
    }
}
