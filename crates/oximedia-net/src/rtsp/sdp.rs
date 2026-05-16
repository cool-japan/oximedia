//! Minimal SDP parser sufficient for RTSP DESCRIBE responses (RFC 8866, formerly 4566).
//!
//! SDP is a line-oriented format where each line is `<type>=<value>`. We only
//! parse the subset that an RTSP client genuinely needs to act on:
//! - session-level connection (`c=`) and media-level connection
//! - media descriptions (`m=`) — payload types and transport
//! - `a=rtpmap`, `a=fmtp`, `a=control`, `a=range`
//!
//! Unknown lines are preserved as raw attributes and not treated as errors —
//! real-world cameras send a lot of vendor extensions.

use crate::error::NetError;

/// A complete SDP session description.
#[derive(Debug, Clone, Default)]
pub struct SessionDescription {
    /// Protocol version (`v=`). Always 0 in practice.
    pub version: u8,
    /// Originator (`o=`) raw value.
    pub origin: Option<String>,
    /// Session name (`s=`).
    pub session_name: Option<String>,
    /// Session-level connection (`c=`).
    pub connection: Option<ConnectionInfo>,
    /// Session-level `a=control` aggregate-control URI.
    pub control: Option<String>,
    /// Session-level range, e.g. `npt=0-`.
    pub range: Option<String>,
    /// One entry per `m=` media line.
    pub media: Vec<MediaDescription>,
    /// All session-level attributes (`a=` lines) preserved verbatim.
    pub attributes: Vec<(String, Option<String>)>,
}

/// `c=` line — network type, address type, connection address.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// Network type. Almost always `IN` (Internet).
    pub network_type: String,
    /// Address type, e.g. `IP4`/`IP6`.
    pub address_type: String,
    /// Connection address, possibly with TTL or multicast count suffix.
    pub address: String,
}

/// A single media stream described by `m=` and its attributes.
#[derive(Debug, Clone)]
pub struct MediaDescription {
    /// Media type: `video`, `audio`, `application`, etc.
    pub media: String,
    /// Transport port; the RFC allows a range but we keep the first value.
    pub port: u16,
    /// Transport protocol, e.g. `RTP/AVP` or `RTP/AVP/TCP`.
    pub protocol: String,
    /// Listed RTP payload types (the trailing tokens of `m=`).
    pub formats: Vec<u8>,
    /// Per-stream connection override, if present.
    pub connection: Option<ConnectionInfo>,
    /// `a=control:` — the URL to SETUP this specific stream.
    pub control: Option<String>,
    /// Decoded `a=rtpmap:` entries keyed by payload type.
    pub rtpmaps: Vec<RtpMap>,
    /// `a=fmtp:` lines keyed by payload type.
    pub fmtps: Vec<Fmtp>,
    /// Any other attribute, preserved as `(name, value)`.
    pub attributes: Vec<(String, Option<String>)>,
}

/// `a=rtpmap:<pt> <encoding>/<clock-rate>[/<channels>]`.
#[derive(Debug, Clone)]
pub struct RtpMap {
    /// Payload type number (0–127).
    pub payload_type: u8,
    /// Codec name, e.g. `H264`, `MP4A-LATM`, `PCMU`.
    pub encoding: String,
    /// Clock rate (Hz). Video is typically 90000.
    pub clock_rate: u32,
    /// Channel count for audio, `None` for video.
    pub channels: Option<u8>,
}

/// `a=fmtp:<pt> <params>`.
#[derive(Debug, Clone)]
pub struct Fmtp {
    /// Payload type number.
    pub payload_type: u8,
    /// Everything after the payload type, unparsed (the syntax is per-codec).
    pub params: String,
}

impl SessionDescription {
    /// Parse a complete SDP document.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Parse`] if a line is missing the `=` separator,
    /// or if a known line type (`v=`, `m=`, `c=`, `a=rtpmap`, `a=fmtp`)
    /// has an unparseable value.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::SessionDescription;
    ///
    /// let sdp = "v=0\r\n\
    ///            m=video 0 RTP/AVP 96\r\n\
    ///            a=rtpmap:96 H264/90000\r\n\
    ///            a=control:trackID=1\r\n";
    /// let parsed = SessionDescription::parse(sdp).unwrap();
    /// let video = parsed.video().expect("video track present");
    /// assert_eq!(video.primary_rtpmap().unwrap().encoding, "H264");
    /// ```
    pub fn parse(input: &str) -> Result<Self, NetError> {
        let mut session = Self::default();
        let mut current_media: Option<MediaDescription> = None;

        for (lineno, raw) in input.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let (kind, value) = line.split_once('=').ok_or_else(|| NetError::Parse {
                offset: lineno as u64,
                message: format!("SDP line missing '=': {line:?}"),
            })?;

            match kind {
                "v" => {
                    session.version = value.trim().parse::<u8>().map_err(|e| NetError::Parse {
                        offset: lineno as u64,
                        message: format!("bad version: {e}"),
                    })?;
                }
                "o" => session.origin = Some(value.to_string()),
                "s" => session.session_name = Some(value.to_string()),
                "c" => {
                    let conn = parse_connection(value, lineno)?;
                    if let Some(m) = current_media.as_mut() {
                        m.connection = Some(conn);
                    } else {
                        session.connection = Some(conn);
                    }
                }
                "m" => {
                    if let Some(m) = current_media.take() {
                        session.media.push(m);
                    }
                    current_media = Some(parse_media(value, lineno)?);
                }
                "a" => {
                    let (attr_name, attr_value) = split_attribute(value);
                    handle_attribute(
                        &mut session,
                        current_media.as_mut(),
                        attr_name,
                        attr_value,
                        lineno,
                    )?;
                }
                _ => {
                    // Lines we don't care about (t=, i=, u=, e=, p=, b=, k=, r=, z=)
                    // are intentionally ignored — RTSP playback doesn't need them.
                }
            }
        }
        if let Some(m) = current_media.take() {
            session.media.push(m);
        }
        Ok(session)
    }

    /// Find the first video media block, if any.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::SessionDescription;
    ///
    /// let sdp = "v=0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n";
    /// let parsed = SessionDescription::parse(sdp).unwrap();
    /// assert!(parsed.video().is_some());
    /// assert!(parsed.audio().is_none());
    /// ```
    #[must_use]
    pub fn video(&self) -> Option<&MediaDescription> {
        self.media.iter().find(|m| m.media == "video")
    }

    /// Find the first audio media block, if any.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::SessionDescription;
    ///
    /// let sdp = "v=0\r\nm=audio 0 RTP/AVP 97\r\na=rtpmap:97 MPEG4-GENERIC/48000/2\r\n";
    /// let parsed = SessionDescription::parse(sdp).unwrap();
    /// let audio = parsed.audio().unwrap();
    /// assert_eq!(audio.primary_rtpmap().unwrap().channels, Some(2));
    /// ```
    #[must_use]
    pub fn audio(&self) -> Option<&MediaDescription> {
        self.media.iter().find(|m| m.media == "audio")
    }
}

impl MediaDescription {
    /// Lookup the `a=rtpmap` for the first listed payload type.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::SessionDescription;
    ///
    /// let sdp = "v=0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n";
    /// let parsed = SessionDescription::parse(sdp).unwrap();
    /// let r = parsed.video().unwrap().primary_rtpmap().unwrap();
    /// assert_eq!(r.encoding, "H264");
    /// assert_eq!(r.clock_rate, 90_000);
    /// ```
    #[must_use]
    pub fn primary_rtpmap(&self) -> Option<&RtpMap> {
        let pt = *self.formats.first()?;
        self.rtpmaps.iter().find(|r| r.payload_type == pt)
    }

    /// Lookup `a=fmtp` for the first listed payload type.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::rtsp::SessionDescription;
    ///
    /// let sdp = "v=0\r\n\
    ///            m=video 0 RTP/AVP 96\r\n\
    ///            a=rtpmap:96 H264/90000\r\n\
    ///            a=fmtp:96 profile-level-id=42E01F\r\n";
    /// let parsed = SessionDescription::parse(sdp).unwrap();
    /// let f = parsed.video().unwrap().primary_fmtp().unwrap();
    /// assert!(f.params.contains("profile-level-id=42E01F"));
    /// ```
    #[must_use]
    pub fn primary_fmtp(&self) -> Option<&Fmtp> {
        let pt = *self.formats.first()?;
        self.fmtps.iter().find(|f| f.payload_type == pt)
    }
}

fn parse_connection(value: &str, lineno: usize) -> Result<ConnectionInfo, NetError> {
    let mut parts = value.split_ascii_whitespace();
    let network_type = parts.next().ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "c= missing network type".into(),
    })?;
    let address_type = parts.next().ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "c= missing address type".into(),
    })?;
    let address = parts.next().ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "c= missing address".into(),
    })?;
    Ok(ConnectionInfo {
        network_type: network_type.to_string(),
        address_type: address_type.to_string(),
        address: address.to_string(),
    })
}

fn parse_media(value: &str, lineno: usize) -> Result<MediaDescription, NetError> {
    let mut parts = value.split_ascii_whitespace();
    let media = parts.next().ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "m= missing media type".into(),
    })?;
    let port_field = parts.next().ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "m= missing port".into(),
    })?;
    // `port[/count]` — keep the leading number, drop any "/count".
    let port_str = port_field.split('/').next().unwrap_or(port_field);
    let port = port_str.parse::<u16>().map_err(|e| NetError::Parse {
        offset: lineno as u64,
        message: format!("bad m= port {port_str:?}: {e}"),
    })?;
    let protocol = parts
        .next()
        .ok_or_else(|| NetError::Parse {
            offset: lineno as u64,
            message: "m= missing protocol".into(),
        })?
        .to_string();
    let mut formats = Vec::new();
    for fmt in parts {
        if let Ok(pt) = fmt.parse::<u8>() {
            formats.push(pt);
        }
        // Non-numeric format tokens appear for non-RTP protocols; we skip them
        // rather than error so we can still parse such SDPs for inspection.
    }
    Ok(MediaDescription {
        media: media.to_string(),
        port,
        protocol,
        formats,
        connection: None,
        control: None,
        rtpmaps: Vec::new(),
        fmtps: Vec::new(),
        attributes: Vec::new(),
    })
}

fn split_attribute(value: &str) -> (&str, Option<&str>) {
    match value.split_once(':') {
        Some((name, rest)) => (name, Some(rest)),
        None => (value, None),
    }
}

fn handle_attribute(
    session: &mut SessionDescription,
    media: Option<&mut MediaDescription>,
    name: &str,
    value: Option<&str>,
    lineno: usize,
) -> Result<(), NetError> {
    match name {
        "control" => {
            let v = value.unwrap_or("").to_string();
            if let Some(m) = media {
                m.control = Some(v);
            } else {
                session.control = Some(v);
            }
        }
        "range" => {
            let v = value.unwrap_or("").to_string();
            if media.is_none() {
                session.range = Some(v);
            } else if let Some(m) = media {
                m.attributes.push(("range".into(), Some(v)));
            }
        }
        "rtpmap" => {
            let v = value.ok_or_else(|| NetError::Parse {
                offset: lineno as u64,
                message: "a=rtpmap missing value".into(),
            })?;
            let rtpmap = parse_rtpmap(v, lineno)?;
            if let Some(m) = media {
                m.rtpmaps.push(rtpmap);
            }
        }
        "fmtp" => {
            let v = value.ok_or_else(|| NetError::Parse {
                offset: lineno as u64,
                message: "a=fmtp missing value".into(),
            })?;
            let fmtp = parse_fmtp(v, lineno)?;
            if let Some(m) = media {
                m.fmtps.push(fmtp);
            }
        }
        other => {
            let v = value.map(str::to_string);
            if let Some(m) = media {
                m.attributes.push((other.to_string(), v));
            } else {
                session.attributes.push((other.to_string(), v));
            }
        }
    }
    Ok(())
}

fn parse_rtpmap(value: &str, lineno: usize) -> Result<RtpMap, NetError> {
    let (pt_str, rest) = value.split_once(' ').ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "a=rtpmap missing space".into(),
    })?;
    let payload_type = pt_str.parse::<u8>().map_err(|e| NetError::Parse {
        offset: lineno as u64,
        message: format!("bad rtpmap PT: {e}"),
    })?;
    // `encoding/clock-rate[/channels]`
    let mut parts = rest.split('/');
    let encoding = parts
        .next()
        .ok_or_else(|| NetError::Parse {
            offset: lineno as u64,
            message: "a=rtpmap missing encoding".into(),
        })?
        .to_string();
    let clock_rate = parts
        .next()
        .ok_or_else(|| NetError::Parse {
            offset: lineno as u64,
            message: "a=rtpmap missing clock-rate".into(),
        })?
        .parse::<u32>()
        .map_err(|e| NetError::Parse {
            offset: lineno as u64,
            message: format!("bad rtpmap clock-rate: {e}"),
        })?;
    let channels = parts.next().and_then(|s| s.parse::<u8>().ok());
    Ok(RtpMap {
        payload_type,
        encoding,
        clock_rate,
        channels,
    })
}

fn parse_fmtp(value: &str, lineno: usize) -> Result<Fmtp, NetError> {
    let (pt_str, params) = value.split_once(' ').ok_or_else(|| NetError::Parse {
        offset: lineno as u64,
        message: "a=fmtp missing space".into(),
    })?;
    let payload_type = pt_str.parse::<u8>().map_err(|e| NetError::Parse {
        offset: lineno as u64,
        message: format!("bad fmtp PT: {e}"),
    })?;
    Ok(Fmtp {
        payload_type,
        params: params.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal SDP that real IP cameras produce.
    const CAMERA_SDP: &str = "v=0\r\n\
o=- 1234 1234 IN IP4 192.168.1.10\r\n\
s=Camera Stream\r\n\
c=IN IP4 0.0.0.0\r\n\
t=0 0\r\n\
a=control:*\r\n\
m=video 0 RTP/AVP 96\r\n\
a=rtpmap:96 H264/90000\r\n\
a=fmtp:96 packetization-mode=1; profile-level-id=42E01F; sprop-parameter-sets=Z0LAH9oBQBboQAAAAwBAAAAPI8WLkgA=,aM48gA==\r\n\
a=control:trackID=1\r\n\
m=audio 0 RTP/AVP 0\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=control:trackID=2\r\n";

    #[test]
    fn parses_camera_sdp() {
        let sdp = SessionDescription::parse(CAMERA_SDP).unwrap();
        assert_eq!(sdp.version, 0);
        assert_eq!(sdp.session_name.as_deref(), Some("Camera Stream"));
        assert_eq!(sdp.control.as_deref(), Some("*"));
        assert_eq!(sdp.media.len(), 2);

        let video = sdp.video().expect("video track");
        assert_eq!(video.media, "video");
        assert_eq!(video.protocol, "RTP/AVP");
        assert_eq!(video.formats, vec![96]);
        assert_eq!(video.control.as_deref(), Some("trackID=1"));
        let rtpmap = video.primary_rtpmap().expect("rtpmap");
        assert_eq!(rtpmap.encoding, "H264");
        assert_eq!(rtpmap.clock_rate, 90000);
        assert!(rtpmap.channels.is_none());
        let fmtp = video.primary_fmtp().expect("fmtp");
        assert!(fmtp.params.contains("profile-level-id=42E01F"));

        let audio = sdp.audio().expect("audio track");
        assert_eq!(audio.formats, vec![0]);
        let rtpmap = audio.primary_rtpmap().expect("audio rtpmap");
        assert_eq!(rtpmap.encoding, "PCMU");
        assert_eq!(rtpmap.clock_rate, 8000);
    }

    #[test]
    fn ignores_unknown_attributes() {
        let sdp = "v=0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=x-vendor-flag:true\r\n";
        let parsed = SessionDescription::parse(sdp).unwrap();
        let video = &parsed.media[0];
        assert!(video.attributes.iter().any(|(k, _)| k == "x-vendor-flag"));
    }

    #[test]
    fn rejects_missing_equals() {
        let bad = "v=0\r\nbadline\r\n";
        assert!(SessionDescription::parse(bad).is_err());
    }

    #[test]
    fn audio_channels_parsed() {
        let sdp = "v=0\r\nm=audio 0 RTP/AVP 97\r\na=rtpmap:97 MPEG4-GENERIC/48000/2\r\n";
        let parsed = SessionDescription::parse(sdp).unwrap();
        let m = &parsed.media[0];
        let r = m.primary_rtpmap().unwrap();
        assert_eq!(r.encoding, "MPEG4-GENERIC");
        assert_eq!(r.clock_rate, 48000);
        assert_eq!(r.channels, Some(2));
    }

    #[test]
    fn media_level_connection_overrides_session() {
        let sdp = "v=0\r\nc=IN IP4 0.0.0.0\r\nm=video 0 RTP/AVP 96\r\nc=IN IP4 239.0.0.1/127\r\na=rtpmap:96 H264/90000\r\n";
        let parsed = SessionDescription::parse(sdp).unwrap();
        let v = &parsed.media[0];
        assert!(v.connection.is_some());
        assert_eq!(v.connection.as_ref().unwrap().address, "239.0.0.1/127");
    }
}
