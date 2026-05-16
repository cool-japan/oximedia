//! Integration tests for the RTSP client.
//!
//! These tests exercise the public API surface as a downstream user would,
//! rather than reaching into module internals. Each test spawns a small
//! tokio TCP listener that plays back a scripted server response and
//! asserts that the client's public state machine drives it correctly.
//!
//! Scenarios covered:
//! - Full happy-path session: OPTIONS → DESCRIBE → SETUP → PLAY → packet → TEARDOWN
//! - 401 Unauthorized challenge / Digest retry
//! - URL parsing → SDP control resolution → SETUP request-URI correctness
//! - SDP parsing surfaces RTP payload metadata that callers need
//! - Sequence-tracker behavior over a stream of incoming RTP packets

use std::time::Duration;

use oximedia_net::rtsp::{
    Challenge, ClientConfig, Credentials, Method, RtpPacket, RtspClient, RtspUrl, SequenceTracker,
    ServerEvent, SessionDescription, SetupTransport,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Spawn a scripted fake RTSP server. The server reads a chunk, then
/// writes the next scripted response. Repeats until the script is empty.
async fn spawn_scripted_server(script: Vec<Vec<u8>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        for chunk in script {
            // For interleaved-only chunks (no preceding request), we still
            // do a single non-blocking read attempt to keep ordering sane.
            let _ = stream.read(&mut buf).await;
            if stream.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = stream.shutdown().await;
    });
    port
}

#[tokio::test]
async fn full_session_happy_path() {
    let sdp_body = "v=0\r\n\
                    o=- 0 0 IN IP4 0.0.0.0\r\n\
                    s=test\r\n\
                    c=IN IP4 0.0.0.0\r\n\
                    a=control:*\r\n\
                    m=video 0 RTP/AVP 96\r\n\
                    a=rtpmap:96 H264/90000\r\n\
                    a=control:trackID=1\r\n";

    let options_resp = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nPublic: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN\r\n\r\n".to_vec();
    let describe_resp = format!(
        "RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
        sdp_body.len(),
        sdp_body
    )
    .into_bytes();
    let setup_resp = b"RTSP/1.0 200 OK\r\nCSeq: 3\r\nSession: SESS1234;timeout=60\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n".to_vec();
    let play_resp = b"RTSP/1.0 200 OK\r\nCSeq: 4\r\nSession: SESS1234\r\n\r\n".to_vec();
    let teardown_resp = b"RTSP/1.0 200 OK\r\nCSeq: 5\r\nSession: SESS1234\r\n\r\n".to_vec();

    let port = spawn_scripted_server(vec![
        options_resp,
        describe_resp,
        setup_resp,
        play_resp,
        teardown_resp,
    ])
    .await;

    let url = format!("rtsp://127.0.0.1:{port}/stream");
    let mut c = RtspClient::connect(&url).await.expect("connect");

    let methods = c.options().await.expect("OPTIONS");
    assert!(methods.contains(&Method::Describe));
    assert!(methods.contains(&Method::Setup));
    assert!(methods.contains(&Method::Play));

    let sdp = c.describe().await.expect("DESCRIBE");
    let video = sdp.video().expect("video track in SDP");
    assert_eq!(video.primary_rtpmap().unwrap().encoding, "H264");

    let control = video.control.as_deref().unwrap_or("");
    let s = c
        .setup(control, &SetupTransport::tcp_interleaved(0))
        .await
        .expect("SETUP");
    assert_eq!(s.session, "SESS1234");
    assert_eq!(s.timeout, 60);
    assert_eq!(c.session(), Some("SESS1234"));

    c.play().await.expect("PLAY");
    c.teardown().await.expect("TEARDOWN");
    assert!(c.session().is_none());
}

#[tokio::test]
async fn digest_auth_retries_on_401() {
    // The server: first request returns 401 with a Digest challenge.
    // The client must parse, retry the same logical operation with an
    // Authorization header, and succeed on the second attempt.
    let challenge = "Digest realm=\"cam\", nonce=\"abc123\", algorithm=MD5, qop=\"auth\"";
    let unauthorized = format!(
        "RTSP/1.0 401 Unauthorized\r\nCSeq: 1\r\nWWW-Authenticate: {challenge}\r\n\r\n"
    )
    .into_bytes();
    let ok = b"RTSP/1.0 200 OK\r\nCSeq: 2\r\nPublic: OPTIONS\r\n\r\n".to_vec();

    let port = spawn_scripted_server(vec![unauthorized, ok]).await;
    let url = format!("rtsp://admin:hunter2@127.0.0.1:{port}/stream");
    let mut c = RtspClient::connect(&url).await.expect("connect");

    // The OPTIONS call must internally re-issue with Authorization and
    // ultimately resolve with the 200 response.
    let methods = c.options().await.expect("OPTIONS after retry");
    assert_eq!(methods, vec![Method::Options]);
}

#[tokio::test]
async fn setup_target_uri_is_resolved_from_sdp_control() {
    // Per RFC 2326 §C.1.1 the SETUP target is `base + a=control` when the
    // control value is relative. We verify the client constructs the right
    // target by sniffing the bytes the server receives.
    let sdp_body = "v=0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=42\r\n";
    let describe_resp = format!(
        "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
        sdp_body.len(),
        sdp_body
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let captured = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        // First request: DESCRIBE
        let _ = stream.read(&mut buf).await.unwrap();
        stream.write_all(describe_resp.as_bytes()).await.unwrap();
        // Second request: SETUP — capture its bytes and reply with 200.
        let n = stream.read(&mut buf).await.unwrap();
        let setup_bytes = buf[..n].to_vec();
        let setup_resp = b"RTSP/1.0 200 OK\r\nCSeq: 2\r\nSession: S\r\nTransport: RTP/AVP/TCP;interleaved=0-1\r\n\r\n";
        stream.write_all(setup_resp).await.unwrap();
        setup_bytes
    });

    let url = format!("rtsp://127.0.0.1:{port}/path");
    let mut c = RtspClient::connect(&url).await.unwrap();
    let sdp = c.describe().await.unwrap();
    let control = sdp.video().unwrap().control.as_deref().unwrap();
    c.setup(control, &SetupTransport::tcp_interleaved(0))
        .await
        .unwrap();

    let setup_bytes = captured.await.unwrap();
    let setup_text = std::str::from_utf8(&setup_bytes).unwrap();
    // The request line must reference the resolved control URI.
    assert!(
        setup_text.starts_with("SETUP rtsp://127.0.0.1:"),
        "request line was: {setup_text:?}"
    );
    assert!(
        setup_text.contains("/path/trackID=42 RTSP/1.0\r\n"),
        "expected resolved path /path/trackID=42 in: {setup_text:?}"
    );
}

#[tokio::test]
async fn interleaved_packets_delivered_in_order() {
    // The server sends three RTP packets after the SETUP response.
    // Verify they arrive on the client's `next_event` queue in the order
    // they were sent and that the SequenceTracker reports no loss.
    fn interleaved(channel: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![b'$', channel];
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn rtp(seq: u16, payload: &[u8]) -> Vec<u8> {
        let mut buf = vec![0x80u8, 0x60u8];
        buf.extend_from_slice(&seq.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes()); // timestamp
        buf.extend_from_slice(&0u32.to_be_bytes()); // ssrc
        buf.extend_from_slice(payload);
        buf
    }

    let mut setup_then_pkts = Vec::new();
    setup_then_pkts
        .extend_from_slice(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: S\r\nTransport: RTP/AVP/TCP;interleaved=0-1\r\n\r\n");
    setup_then_pkts.extend_from_slice(&interleaved(0, &rtp(100, b"P1")));
    setup_then_pkts.extend_from_slice(&interleaved(0, &rtp(101, b"P2")));
    setup_then_pkts.extend_from_slice(&interleaved(0, &rtp(102, b"P3")));

    let port = spawn_scripted_server(vec![setup_then_pkts]).await;
    let url = format!("rtsp://127.0.0.1:{port}/stream");
    let mut c = RtspClient::connect(&url).await.unwrap();
    c.setup("trackID=1", &SetupTransport::tcp_interleaved(0))
        .await
        .unwrap();

    let mut tracker = SequenceTracker::new();
    let mut seen_payloads: Vec<Vec<u8>> = Vec::new();
    for _ in 0..3 {
        match c.next_event().await.unwrap() {
            ServerEvent::Packet(p) => {
                let rtp = RtpPacket::parse(&p.data).unwrap();
                tracker.observe(rtp.sequence);
                seen_payloads.push(rtp.payload.to_vec());
            }
            ServerEvent::Message(_) => panic!("expected RTP packets, not RTSP messages"),
        }
    }

    assert_eq!(
        seen_payloads,
        vec![b"P1".to_vec(), b"P2".to_vec(), b"P3".to_vec()]
    );
    assert_eq!(tracker.received, 3);
    assert_eq!(tracker.lost, 0);
    assert_eq!(tracker.reordered, 0);
    assert_eq!(tracker.duplicates, 0);
}

#[tokio::test]
async fn connect_timeout_returns_timeout_error() {
    use oximedia_net::error::NetError;

    // Use the documented TEST-NET-1 address (RFC 5737) which won't route.
    // 192.0.2.0/24 is reserved for documentation and never reachable, so
    // the TCP SYN will hang until the timeout fires deterministically.
    let cfg = ClientConfig {
        io_timeout: Duration::from_millis(150),
        ..ClientConfig::default()
    };
    let result = RtspClient::connect_with("rtsp://192.0.2.1:554/x", cfg).await;
    match result {
        Err(NetError::Timeout(_)) => { /* expected */ }
        Err(other) => panic!("expected Timeout, got {other:?}"),
        Ok(_) => panic!("connect to TEST-NET-1 must not succeed"),
    }
}

#[test]
fn url_parsing_preserves_userinfo_for_auth_and_strips_for_wire() {
    // The credentials in the URL must drive 401 retries, but never appear
    // on the wire (they belong in Authorization headers only).
    let u = RtspUrl::parse("rtsp://admin:hunter2@cam.local:554/live").unwrap();
    assert_eq!(
        u.userinfo,
        Some(("admin".to_string(), "hunter2".to_string()))
    );
    // request_uri is the form used in the request line — must not include
    // userinfo (and must omit the default port).
    assert_eq!(u.request_uri(), "rtsp://cam.local/live");
    // authority is the form passed to TcpStream::connect.
    assert_eq!(u.authority(), "cam.local:554");
}

#[test]
fn sdp_round_trip_surfaces_h264_parameters() {
    // A realistic SDP block that an IP camera would send back. The
    // integration point under test: a downstream depacketizer must be
    // able to obtain payload type, clock rate, and the H.264 sprop
    // parameter sets without re-parsing the SDP itself.
    let sdp = "v=0\r\n\
               o=- 0 0 IN IP4 0.0.0.0\r\n\
               s=Camera\r\n\
               c=IN IP4 0.0.0.0\r\n\
               m=video 0 RTP/AVP 96\r\n\
               a=rtpmap:96 H264/90000\r\n\
               a=fmtp:96 packetization-mode=1; profile-level-id=42E01F; sprop-parameter-sets=Z0LAH9oBQBboQAAAAwBAAAAPI8WLkgA=,aM48gA==\r\n\
               a=control:trackID=1\r\n";

    let parsed = SessionDescription::parse(sdp).unwrap();
    let video = parsed.video().unwrap();
    assert_eq!(video.formats, vec![96]);
    let r = video.primary_rtpmap().unwrap();
    assert_eq!(r.payload_type, 96);
    assert_eq!(r.encoding, "H264");
    assert_eq!(r.clock_rate, 90_000);
    let f = video.primary_fmtp().unwrap();
    assert!(f.params.contains("profile-level-id=42E01F"));
    assert!(f.params.contains("sprop-parameter-sets="));
}

#[test]
fn digest_challenge_round_trips_through_credentials() {
    // The auth pieces (challenge + credentials + per-request authorization)
    // must compose to produce a valid HTTP Digest Authorization header.
    let c = Challenge::parse(
        "Digest realm=\"cam\", nonce=\"deadbeef\", algorithm=MD5, qop=\"auth\"",
    )
    .unwrap();
    let creds = Credentials {
        username: "admin".into(),
        password: "secret".into(),
    };
    let header = c.build_authorization(&creds, "DESCRIBE", "rtsp://cam/live", 1, "cn-abc");
    assert!(header.starts_with("Digest "));
    assert!(header.contains("username=\"admin\""));
    assert!(header.contains("realm=\"cam\""));
    assert!(header.contains("nonce=\"deadbeef\""));
    assert!(header.contains("uri=\"rtsp://cam/live\""));
    assert!(header.contains("qop=auth"));
    assert!(header.contains("nc=00000001"));
    assert!(header.contains("cnonce=\"cn-abc\""));
    assert!(header.contains("response=\""));
}
