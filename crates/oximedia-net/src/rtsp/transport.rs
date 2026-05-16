//! TCP-interleaved transport framing (RFC 2326 §10.12).
//!
//! When a client requests `Transport: RTP/AVP/TCP;interleaved=N-N+1`, RTP and
//! RTCP packets are framed inline on the same TCP connection as the RTSP
//! request/response stream. The framing is:
//!
//! ```text
//! +------+---------+--------+-----------+
//! | 0x24 | channel | length |   data    |
//! +------+---------+--------+-----------+
//!   1 B    1 B       2 B BE   length B
//! ```
//!
//! RTSP messages always start with `RTSP/1.0` (response) or a method name
//! (request) — never `0x24` — so a single peek of the next byte distinguishes
//! the two framings.

/// One interleaved RTP or RTCP packet pulled off the TCP connection.
#[derive(Debug, Clone)]
pub struct InterleavedPacket {
    /// Channel ID (matches what was negotiated in `Transport: interleaved=`).
    pub channel: u8,
    /// Packet payload (RTP for even channels, RTCP for odd channels by convention).
    pub data: Vec<u8>,
}

/// Status of a non-blocking frame-decode attempt.
#[derive(Debug)]
pub enum FrameStatus {
    /// More bytes needed before a complete frame is available.
    NeedMore,
    /// Next chunk in the buffer is a complete interleaved packet.
    Interleaved {
        /// Bytes consumed (header + payload).
        consumed: usize,
        /// The parsed packet.
        packet: InterleavedPacket,
    },
    /// Next chunk is an RTSP message — caller should run the RTSP parser.
    RtspMessage,
}

/// Attempt to decode the next frame from `buf` non-destructively.
///
/// The buffer is left untouched on `NeedMore`; on a successful `Interleaved`
/// decode the caller must drain `consumed` bytes off the front. `RtspMessage`
/// is a hint that the caller should hand the buffer to the RTSP parser.
///
/// # Example
///
/// ```
/// use oximedia_net::rtsp::{next_frame, encode_interleaved, FrameStatus};
///
/// let wire = encode_interleaved(0, b"rtp-payload");
/// match next_frame(&wire) {
///     FrameStatus::Interleaved { consumed, packet } => {
///         assert_eq!(consumed, wire.len());
///         assert_eq!(packet.channel, 0);
///         assert_eq!(packet.data, b"rtp-payload");
///     }
///     _ => unreachable!(),
/// }
///
/// // An RTSP response in the same buffer is signaled separately.
/// assert!(matches!(next_frame(b"RTSP/1.0"), FrameStatus::RtspMessage));
/// ```
#[must_use]
pub fn next_frame(buf: &[u8]) -> FrameStatus {
    if buf.is_empty() {
        return FrameStatus::NeedMore;
    }
    if buf[0] != b'$' {
        return FrameStatus::RtspMessage;
    }
    if buf.len() < 4 {
        return FrameStatus::NeedMore;
    }
    let channel = buf[1];
    let length = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let total = 4 + length;
    if buf.len() < total {
        return FrameStatus::NeedMore;
    }
    FrameStatus::Interleaved {
        consumed: total,
        packet: InterleavedPacket {
            channel,
            data: buf[4..total].to_vec(),
        },
    }
}

/// Encode an interleaved packet to the wire format.
///
/// # Example
///
/// ```
/// use oximedia_net::rtsp::encode_interleaved;
///
/// let wire = encode_interleaved(3, b"abc");
/// assert_eq!(wire, b"$\x03\x00\x03abc");
/// ```
#[must_use]
pub fn encode_interleaved(channel: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.push(b'$');
    out.push(channel);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_needs_more() {
        assert!(matches!(next_frame(&[]), FrameStatus::NeedMore));
    }

    #[test]
    fn rtsp_byte_signals_message() {
        assert!(matches!(next_frame(b"RTSP/1.0 "), FrameStatus::RtspMessage));
    }

    #[test]
    fn partial_interleaved_header_needs_more() {
        assert!(matches!(next_frame(b"$\x00\x10"), FrameStatus::NeedMore));
    }

    #[test]
    fn partial_interleaved_payload_needs_more() {
        let mut buf = vec![b'$', 0, 0, 8];
        buf.extend_from_slice(&[1, 2, 3]); // only 3 of 8 payload bytes
        assert!(matches!(next_frame(&buf), FrameStatus::NeedMore));
    }

    #[test]
    fn parses_complete_interleaved_packet() {
        let payload = b"hello-rtp-payload";
        let mut buf = vec![b'$', 7];
        buf.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        buf.extend_from_slice(payload);
        match next_frame(&buf) {
            FrameStatus::Interleaved { consumed, packet } => {
                assert_eq!(consumed, 4 + payload.len());
                assert_eq!(packet.channel, 7);
                assert_eq!(packet.data, payload);
            }
            other => panic!("expected interleaved, got {other:?}"),
        }
    }

    #[test]
    fn encode_round_trips() {
        let enc = encode_interleaved(3, b"abc");
        match next_frame(&enc) {
            FrameStatus::Interleaved { consumed, packet } => {
                assert_eq!(consumed, enc.len());
                assert_eq!(packet.channel, 3);
                assert_eq!(packet.data, b"abc");
            }
            _ => panic!("expected interleaved"),
        }
    }
}
