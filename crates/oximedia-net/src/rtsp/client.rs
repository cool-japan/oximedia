//! Async RTSP client (RFC 2326).
//!
//! Drives the standard control flow:
//!
//! ```text
//! connect → OPTIONS → DESCRIBE → SETUP (per track) → PLAY
//!                                                       │
//!                                                       ▼
//!                                       loop { recv_packet() }
//!                                                       │
//!                                                       ▼
//!                                                  TEARDOWN
//! ```
//!
//! Transport is TCP-interleaved by default (the only mode that works through
//! NAT without external port mapping). UDP transport is a future addition.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::auth::{Challenge, Credentials};
use super::message::{try_parse_response, Method, ParseStatus, Request, Response};
use super::sdp::SessionDescription;
use super::transport::{next_frame, FrameStatus, InterleavedPacket};
use super::url::RtspUrl;
use crate::error::NetError;

/// Default RTSP TCP port (RFC 2326).
pub const DEFAULT_RTSP_PORT: u16 = 554;

/// Default user-agent string sent on every request.
pub const USER_AGENT: &str = concat!("oximedia-net/", env!("CARGO_PKG_VERSION"));

/// Configuration for [`RtspClient::connect_with`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Read/write timeout per single I/O operation.
    pub io_timeout: Duration,
    /// User-Agent header value.
    pub user_agent: String,
    /// Credentials, if known up front. Otherwise the client will fall back to
    /// the userinfo in the URL on a 401.
    pub credentials: Option<Credentials>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            io_timeout: Duration::from_secs(15),
            user_agent: USER_AGENT.to_string(),
            credentials: None,
        }
    }
}

/// SETUP request parameters.
#[derive(Debug, Clone)]
pub struct SetupTransport {
    /// Even channel id used for RTP frames.
    pub interleaved_rtp: u8,
    /// Odd channel id (= `interleaved_rtp + 1`) used for RTCP frames.
    pub interleaved_rtcp: u8,
}

impl SetupTransport {
    /// Build a transport using channels `(n, n+1)`.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::SetupTransport;
    /// let t = SetupTransport::tcp_interleaved(0);
    /// assert_eq!(t.interleaved_rtp, 0);
    /// assert_eq!(t.interleaved_rtcp, 1);
    /// ```
    #[must_use]
    pub fn tcp_interleaved(rtp_channel: u8) -> Self {
        Self {
            interleaved_rtp: rtp_channel,
            interleaved_rtcp: rtp_channel.wrapping_add(1),
        }
    }

    fn header_value(&self) -> String {
        format!(
            "RTP/AVP/TCP;unicast;interleaved={}-{}",
            self.interleaved_rtp, self.interleaved_rtcp
        )
    }
}

/// Response payload from a successful SETUP.
#[derive(Debug, Clone)]
pub struct SetupResponse {
    /// Session token returned by the server (echoed on subsequent requests).
    pub session: String,
    /// Server-suggested session timeout in seconds (default 60 if absent).
    pub timeout: u64,
    /// `Transport:` header echoed back, unparsed.
    pub transport: String,
}

/// One asynchronous event delivered from the server.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// An interleaved RTP/RTCP packet.
    Packet(InterleavedPacket),
    /// An out-of-band RTSP message (rare — typically `ANNOUNCE` from the server).
    Message(Response),
}

/// Async RTSP client.
pub struct RtspClient {
    stream: TcpStream,
    url: RtspUrl,
    cfg: ClientConfig,
    cseq: u32,
    session: Option<String>,
    session_timeout: u64,
    challenge: Option<Challenge>,
    nc: u32,
    rx_buf: Vec<u8>,
    pending_events: VecDeque<ServerEvent>,
}

impl RtspClient {
    /// Connect with default configuration.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InvalidUrl`] if the URL is malformed,
    /// [`NetError::Timeout`] if the TCP connect exceeds the default
    /// 15-second timeout, or [`NetError::Io`] on socket errors.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::RtspClient;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let _client = RtspClient::connect("rtsp://camera.local/live").await?;
    /// # Ok(()) }
    /// ```
    pub async fn connect(url: &str) -> Result<Self, NetError> {
        Self::connect_with(url, ClientConfig::default()).await
    }

    /// Connect with explicit configuration.
    ///
    /// # Errors
    ///
    /// Same as [`connect`](Self::connect).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use oximedia_net::rtsp::{ClientConfig, Credentials, RtspClient};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let cfg = ClientConfig {
    ///     io_timeout: Duration::from_secs(5),
    ///     credentials: Some(Credentials {
    ///         username: "admin".into(),
    ///         password: "secret".into(),
    ///     }),
    ///     ..ClientConfig::default()
    /// };
    /// let _c = RtspClient::connect_with("rtsp://camera/live", cfg).await?;
    /// # Ok(()) }
    /// ```
    pub async fn connect_with(url: &str, cfg: ClientConfig) -> Result<Self, NetError> {
        let parsed = RtspUrl::parse(url)?;
        let stream = tokio::time::timeout(
            cfg.io_timeout,
            TcpStream::connect(parsed.authority()),
        )
        .await
        .map_err(|_| NetError::Timeout(format!("connect to {}", parsed.authority())))?
        .map_err(NetError::Io)?;
        Ok(Self {
            stream,
            url: parsed,
            cfg,
            cseq: 0,
            session: None,
            session_timeout: 60,
            challenge: None,
            nc: 0,
            rx_buf: Vec::with_capacity(8192),
            pending_events: VecDeque::new(),
        })
    }

    /// Parsed URL the client connected to.
    #[must_use]
    pub fn url(&self) -> &RtspUrl {
        &self.url
    }

    /// Current session token, if PLAY/SETUP has been issued.
    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Server-advertised session timeout in seconds.
    #[must_use]
    pub fn session_timeout(&self) -> u64 {
        self.session_timeout
    }

    /// Send `OPTIONS *` and return the parsed `Public:` method list.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response,
    /// [`NetError::Timeout`] on read/write timeout, or
    /// [`NetError::Connection`] if the server closes mid-exchange.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::{Method, RtspClient};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// let methods = c.options().await?;
    /// assert!(methods.contains(&Method::Describe));
    /// # Ok(()) }
    /// ```
    pub async fn options(&mut self) -> Result<Vec<Method>, NetError> {
        let resp = self.request(Method::Options, &self.url.request_uri(), None, &[]).await?;
        if !resp.is_success() {
            return Err(resp.into_http_error());
        }
        let methods = resp
            .headers
            .get("Public")
            .map(|s| {
                s.split(',')
                    .filter_map(|m| Method::parse(m.trim()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Ok(methods)
    }

    /// Send `DESCRIBE`, accept `application/sdp`, and parse the body.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response,
    /// [`NetError::Protocol`] if the body is not valid UTF-8, or
    /// [`NetError::Parse`] if the SDP body fails to parse.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::RtspClient;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// let sdp = c.describe().await?;
    /// if let Some(video) = sdp.video() {
    ///     println!("video codec: {:?}", video.primary_rtpmap().map(|r| &r.encoding));
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn describe(&mut self) -> Result<SessionDescription, NetError> {
        let resp = self
            .request(
                Method::Describe,
                &self.url.request_uri(),
                None,
                &[("Accept", "application/sdp")],
            )
            .await?;
        if !resp.is_success() {
            return Err(resp.into_http_error());
        }
        let text = std::str::from_utf8(&resp.body)
            .map_err(|e| NetError::Protocol(format!("non-UTF-8 SDP body: {e}")))?;
        SessionDescription::parse(text)
    }

    /// SETUP a single track using its `a=control:` URL.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response or
    /// [`NetError::Protocol`] if the response is missing the mandatory
    /// `Session:` header.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::{RtspClient, SetupTransport};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// let sdp = c.describe().await?;
    /// let video = sdp.video().expect("video track");
    /// let control = video.control.as_deref().unwrap_or("trackID=1");
    /// let s = c.setup(control, &SetupTransport::tcp_interleaved(0)).await?;
    /// println!("session={} timeout={}s", s.session, s.timeout);
    /// # Ok(()) }
    /// ```
    pub async fn setup(
        &mut self,
        control_url: &str,
        transport: &SetupTransport,
    ) -> Result<SetupResponse, NetError> {
        let target = self.url.resolve_control(control_url);
        let resp = self
            .request(
                Method::Setup,
                &target,
                None,
                &[("Transport", transport.header_value().as_str())],
            )
            .await?;
        if !resp.is_success() {
            return Err(resp.into_http_error());
        }
        let session_header = resp
            .headers
            .get("Session")
            .ok_or_else(|| NetError::Protocol("SETUP response missing Session header".into()))?
            .to_string();
        let (session_id, timeout) = parse_session_header(&session_header);
        self.session = Some(session_id.clone());
        self.session_timeout = timeout;
        Ok(SetupResponse {
            session: session_id,
            timeout,
            transport: resp.headers.get("Transport").unwrap_or("").to_string(),
        })
    }

    /// PLAY the aggregate session.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::{RtspClient, SetupTransport};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// c.setup("trackID=1", &SetupTransport::tcp_interleaved(0)).await?;
    /// c.play().await?;
    /// # Ok(()) }
    /// ```
    pub async fn play(&mut self) -> Result<(), NetError> {
        let target = self.url.request_uri();
        let resp = self.request(Method::Play, &target, None, &[]).await?;
        if !resp.is_success() {
            return Err(resp.into_http_error());
        }
        Ok(())
    }

    /// PAUSE the aggregate session.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::RtspClient;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// // ... after SETUP + PLAY ...
    /// c.pause().await?;
    /// # Ok(()) }
    /// ```
    pub async fn pause(&mut self) -> Result<(), NetError> {
        let target = self.url.request_uri();
        let resp = self.request(Method::Pause, &target, None, &[]).await?;
        if !resp.is_success() {
            return Err(resp.into_http_error());
        }
        Ok(())
    }

    /// Send a keepalive request (`GET_PARAMETER` if supported, falling back to
    /// `OPTIONS`). Call periodically to keep the session from timing out.
    ///
    /// Send-rate guidance: half the value of [`session_timeout`](Self::session_timeout).
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response other than 405.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use oximedia_net::rtsp::RtspClient;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// // ... SETUP + PLAY ...
    /// let interval = Duration::from_secs(c.session_timeout() / 2);
    /// // tokio::time::sleep(interval).await;
    /// c.keepalive().await?;
    /// # let _ = interval;
    /// # Ok(()) }
    /// ```
    pub async fn keepalive(&mut self) -> Result<(), NetError> {
        let target = self.url.request_uri();
        let resp = self.request(Method::GetParameter, &target, None, &[]).await?;
        if resp.is_success() {
            return Ok(());
        }
        if resp.status == 405 {
            // 405 Method Not Allowed → server doesn't support GET_PARAMETER;
            // OPTIONS is universally allowed.
            let _ = self.options().await?;
            return Ok(());
        }
        Err(resp.into_http_error())
    }

    /// TEARDOWN and drop the session id.
    ///
    /// No-op if no session has been established yet.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] on a non-2xx response.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::RtspClient;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// // ... full lifecycle ...
    /// c.teardown().await?;
    /// assert!(c.session().is_none());
    /// # Ok(()) }
    /// ```
    pub async fn teardown(&mut self) -> Result<(), NetError> {
        if self.session.is_none() {
            return Ok(());
        }
        let target = self.url.request_uri();
        let resp = self.request(Method::Teardown, &target, None, &[]).await?;
        self.session = None;
        if !resp.is_success() {
            return Err(resp.into_http_error());
        }
        Ok(())
    }

    /// Wait for the next server-pushed event (RTP packet or unsolicited message).
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Timeout`] / [`NetError::Io`] on read failure,
    /// or [`NetError::Connection`] if the server closes the TCP socket.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_net::rtsp::{RtpPacket, RtspClient, ServerEvent, SetupTransport};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut c = RtspClient::connect("rtsp://cam/live").await?;
    /// c.setup("trackID=1", &SetupTransport::tcp_interleaved(0)).await?;
    /// c.play().await?;
    /// match c.next_event().await? {
    ///     ServerEvent::Packet(pkt) => {
    ///         let rtp = RtpPacket::parse(&pkt.data)?;
    ///         println!("ch={} seq={} pt={}", pkt.channel, rtp.sequence, rtp.payload_type);
    ///     }
    ///     ServerEvent::Message(_) => { /* rare ANNOUNCE etc. */ }
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn next_event(&mut self) -> Result<ServerEvent, NetError> {
        if let Some(ev) = self.pending_events.pop_front() {
            return Ok(ev);
        }
        loop {
            if let Some(ev) = self.try_drain_frame()? {
                return Ok(ev);
            }
            self.read_more().await?;
        }
    }

    // ─── internals ─────────────────────────────────────────────────────────

    async fn request(
        &mut self,
        method: Method,
        target: &str,
        body: Option<Vec<u8>>,
        extra_headers: &[(&str, &str)],
    ) -> Result<Response, NetError> {
        let resp = self
            .send_once(method, target, body.clone(), extra_headers)
            .await?;
        if resp.is_unauthorized() && self.try_pick_up_challenge(&resp).is_some() {
            // Retry exactly once with the freshly-stored challenge.
            return self.send_once(method, target, body, extra_headers).await;
        }
        Ok(resp)
    }

    async fn send_once(
        &mut self,
        method: Method,
        target: &str,
        body: Option<Vec<u8>>,
        extra_headers: &[(&str, &str)],
    ) -> Result<Response, NetError> {
        self.cseq += 1;
        let mut req = Request::new(method, target, self.cseq)
            .with_header("User-Agent", self.cfg.user_agent.clone());

        for (k, v) in extra_headers {
            req = req.with_header(k, *v);
        }
        if let Some(body) = body {
            req = req.with_body(body);
        }
        // Auth header is added last so it shadows anything else.
        if let Some(auth) = self.build_auth_header(method, target) {
            req.headers.insert("Authorization", auth);
        }
        if let Some(s) = &self.session {
            // Only set if the caller didn't already; check by lowercase key.
            if req.headers.get("Session").is_none() {
                req.headers.insert("Session", s.clone());
            }
        }

        let wire = req.encode();
        self.write_all(&wire).await?;

        // Drain interleaved frames and wait for the matching response.
        loop {
            if let Some(ev) = self.try_drain_frame()? {
                match ev {
                    ServerEvent::Packet(_) => self.pending_events.push_back(ev),
                    ServerEvent::Message(resp) => return Ok(resp),
                }
                continue;
            }
            self.read_more().await?;
        }
    }

    fn try_pick_up_challenge(&mut self, resp: &Response) -> Option<()> {
        let challenge_header = resp.headers.get("WWW-Authenticate")?;
        let challenge = Challenge::parse(challenge_header).ok()?;
        // Reset nonce-count whenever we adopt a new challenge.
        self.nc = 0;
        self.challenge = Some(challenge);
        Some(())
    }

    fn build_auth_header(&mut self, method: Method, target: &str) -> Option<String> {
        let challenge = self.challenge.as_ref()?;
        let creds = self
            .cfg
            .credentials
            .clone()
            .or_else(|| {
                self.url.userinfo.clone().map(|(u, p)| Credentials {
                    username: u,
                    password: p,
                })
            })?;
        self.nc += 1;
        let cnonce = generate_cnonce(self.cseq);
        Some(challenge.build_authorization(&creds, method.as_str(), target, self.nc, &cnonce))
    }

    async fn read_more(&mut self) -> Result<(), NetError> {
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout(self.cfg.io_timeout, self.stream.read(&mut chunk))
            .await
            .map_err(|_| NetError::Timeout("RTSP read".into()))?
            .map_err(NetError::Io)?;
        if n == 0 {
            return Err(NetError::Connection("server closed connection".into()));
        }
        self.rx_buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    async fn write_all(&mut self, data: &[u8]) -> Result<(), NetError> {
        tokio::time::timeout(self.cfg.io_timeout, self.stream.write_all(data))
            .await
            .map_err(|_| NetError::Timeout("RTSP write".into()))?
            .map_err(NetError::Io)?;
        self.stream
            .flush()
            .await
            .map_err(NetError::Io)?;
        Ok(())
    }

    /// Consume any complete frame from `rx_buf`. Returns `Some(_)` if either
    /// an interleaved packet or a full RTSP response is available.
    fn try_drain_frame(&mut self) -> Result<Option<ServerEvent>, NetError> {
        match next_frame(&self.rx_buf) {
            FrameStatus::NeedMore => Ok(None),
            FrameStatus::Interleaved { consumed, packet } => {
                self.rx_buf.drain(..consumed);
                Ok(Some(ServerEvent::Packet(packet)))
            }
            FrameStatus::RtspMessage => match try_parse_response(&self.rx_buf)? {
                ParseStatus::NeedMore => Ok(None),
                ParseStatus::Parsed { consumed, response } => {
                    self.rx_buf.drain(..consumed);
                    Ok(Some(ServerEvent::Message(response)))
                }
            },
        }
    }
}

/// Parse a `Session: <id>[;timeout=N]` header.
fn parse_session_header(value: &str) -> (String, u64) {
    let mut parts = value.split(';');
    let id = parts.next().unwrap_or("").trim().to_string();
    let mut timeout = 60u64;
    for part in parts {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("timeout=") {
            if let Ok(v) = rest.parse::<u64>() {
                timeout = v;
            }
        }
    }
    (id, timeout)
}

/// Build a per-request client nonce. Combines an incrementing counter with a
/// random-ish suffix from the process clock — sufficient for HTTP Digest's
/// uniqueness requirement (the server only checks that nc+cnonce don't repeat
/// against the same server nonce).
fn generate_cnonce(seed: u32) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("{:08x}{:016x}", seed, ts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::transport::encode_interleaved;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    async fn spawn_fake_server(script: Vec<Vec<u8>>) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Naive script player: read up to one line per scripted response,
            // then write the scripted response. Sufficient for protocol shape
            // tests; the message parser is exercised separately.
            let mut buf = [0u8; 2048];
            for chunk in script {
                let _ = stream.read(&mut buf).await;
                stream.write_all(&chunk).await.unwrap();
            }
            let _ = stream.shutdown().await;
        });
        (port, handle)
    }

    #[test]
    fn session_header_parses_timeout() {
        assert_eq!(
            parse_session_header("12345678;timeout=30"),
            ("12345678".to_string(), 30)
        );
        assert_eq!(
            parse_session_header(" abcd "),
            ("abcd".to_string(), 60)
        );
    }

    #[test]
    fn setup_transport_header() {
        let t = SetupTransport::tcp_interleaved(0);
        assert_eq!(t.header_value(), "RTP/AVP/TCP;unicast;interleaved=0-1");
    }

    #[tokio::test]
    async fn options_round_trip_against_fake_server() {
        let response = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nPublic: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN\r\n\r\n".to_vec();
        let (port, _handle) = spawn_fake_server(vec![response]).await;
        let url = format!("rtsp://127.0.0.1:{port}/test");
        let mut c = RtspClient::connect(&url).await.unwrap();
        let methods = c.options().await.unwrap();
        assert!(methods.contains(&Method::Describe));
        assert!(methods.contains(&Method::Setup));
        assert!(methods.contains(&Method::Play));
    }

    #[tokio::test]
    async fn describe_parses_returned_sdp() {
        let body = "v=0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=1\r\n";
        let response = format!(
            "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let (port, _h) = spawn_fake_server(vec![response.into_bytes()]).await;
        let url = format!("rtsp://127.0.0.1:{port}/test");
        let mut c = RtspClient::connect(&url).await.unwrap();
        let sdp = c.describe().await.unwrap();
        let v = sdp.video().unwrap();
        assert_eq!(v.primary_rtpmap().unwrap().encoding, "H264");
        assert_eq!(v.control.as_deref(), Some("trackID=1"));
    }

    #[tokio::test]
    async fn setup_stores_session_and_timeout() {
        let response =
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: ABCD1234;timeout=30\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n";
        let (port, _h) = spawn_fake_server(vec![response.to_vec()]).await;
        let url = format!("rtsp://127.0.0.1:{port}/test");
        let mut c = RtspClient::connect(&url).await.unwrap();
        let r = c
            .setup("trackID=1", &SetupTransport::tcp_interleaved(0))
            .await
            .unwrap();
        assert_eq!(r.session, "ABCD1234");
        assert_eq!(r.timeout, 30);
        assert_eq!(c.session(), Some("ABCD1234"));
        assert_eq!(c.session_timeout(), 30);
    }

    #[tokio::test]
    async fn interleaved_packet_delivered_via_next_event() {
        // Server sends back a SETUP response and then an interleaved packet
        // before the client asks. We expect:
        //   - SETUP completes
        //   - next_event() yields the interleaved packet
        let payload = b"FAKE-RTP-PAYLOAD";
        let mut script = Vec::new();
        script.extend_from_slice(
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: S\r\n\r\n",
        );
        script.push(b'$');
        script.push(0);
        script.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        script.extend_from_slice(payload);

        let (port, _h) = spawn_fake_server(vec![script]).await;
        let url = format!("rtsp://127.0.0.1:{port}/test");
        let mut c = RtspClient::connect(&url).await.unwrap();
        let _ = c
            .setup("trackID=1", &SetupTransport::tcp_interleaved(0))
            .await
            .unwrap();
        let ev = c.next_event().await.unwrap();
        match ev {
            ServerEvent::Packet(p) => {
                assert_eq!(p.channel, 0);
                assert_eq!(p.data, payload);
            }
            _ => panic!("expected packet"),
        }
    }

    #[test]
    fn encode_interleaved_helper_visible() {
        // Re-export sanity — must still build through the module.
        let _ = encode_interleaved(0, b"x");
    }
}
