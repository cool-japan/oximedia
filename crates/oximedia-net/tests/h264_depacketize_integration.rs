//! Integration test: a realistic stream of H.264 RTP packets is fed
//! through `H264Depacketizer` and the output is verified to be correct
//! Annex-B access units.
//!
//! The packets are constructed inline so this test has no dependency on
//! the RTSP module (which lives on a parallel branch); it exercises
//! every supported packetization mode in the orders an IP camera
//! actually sends them:
//!
//!   AU 1: STAP-A(SPS, PPS) → FU-A(IDR, start) → FU-A(IDR, mid) →
//!         FU-A(IDR, end + marker)
//!   AU 2: single NAL(non-IDR, marker)
//!
//! That's the canonical "first IDR + one inter frame" sequence that
//! every H.264 RTP stream begins with — exactly what the upstream
//! consumer (`oximedia-vtb::H264Decoder`) will be fed.

use oximedia_net::depacketize::H264Depacketizer;

/// Minimal RTP-header-trimming helper: returns `(payload, marker)`.
/// We synthesize 12-byte fixed-header packets (V=2, no CC/X/P).
fn rtp_packet(seq: u16, ts: u32, pt: u8, marker: bool, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + payload.len());
    p.push(0x80); // V=2, P=0, X=0, CC=0
    p.push((u8::from(marker) << 7) | (pt & 0x7F));
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ts.to_be_bytes());
    p.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// Extract `(payload, marker)` from a 12-byte-fixed-header RTP packet —
/// the same operation `RtpPacket::parse` performs on the rtsp branch.
fn split_rtp(packet: &[u8]) -> (&[u8], bool) {
    assert!(packet.len() >= 12);
    let marker = packet[1] & 0x80 != 0;
    (&packet[12..], marker)
}

fn stap_a(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![(3u8 << 5) | 24]; // NRI=3, type=24
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

fn fu_a(nal_type: u8, start: bool, end: bool, fragment: &[u8]) -> Vec<u8> {
    let indicator = (3u8 << 5) | 28;
    let mut fu_header = nal_type & 0x1F;
    if start {
        fu_header |= 0x80;
    }
    if end {
        fu_header |= 0x40;
    }
    let mut out = vec![indicator, fu_header];
    out.extend_from_slice(fragment);
    out
}

#[test]
fn ip_camera_first_two_access_units() {
    // Synthetic SPS/PPS/IDR bodies — bytes are arbitrary; this test
    // exercises RTP packetization, not real H.264 decode.
    let sps_body = b"-fake-sps-body-".to_vec();
    let pps_body = b"-fake-pps-body-".to_vec();
    let mut sps_nal = vec![(3u8 << 5) | 7];
    sps_nal.extend_from_slice(&sps_body);
    let mut pps_nal = vec![(3u8 << 5) | 8];
    pps_nal.extend_from_slice(&pps_body);

    let idr_frag_a = b"IDR-AAAAAAAAAAAAAAAAAAAA";
    let idr_frag_b = b"IDR-BBBBBBBBBBBBBBBBBBBB";
    let idr_frag_c = b"IDR-CCCCCCCCCCCCCCCCCCCC";

    let inter_body = b"-non-idr-slice-body-".to_vec();
    // NAL header: F=0, NRI=3, type=1 (non-IDR slice).
    let mut inter_nal = vec![(3u8 << 5) | 1];
    inter_nal.extend_from_slice(&inter_body);

    // The wire-order stream of RTP packets the camera emits.
    let packets = vec![
        // AU 1: SPS + PPS in a single STAP-A (no marker yet, more coming).
        rtp_packet(100, 9000, 96, false, &stap_a(&[&sps_nal, &pps_nal])),
        // AU 1: IDR split into three FU-A fragments.
        rtp_packet(101, 9000, 96, false, &fu_a(5, true, false, idr_frag_a)),
        rtp_packet(102, 9000, 96, false, &fu_a(5, false, false, idr_frag_b)),
        // marker=true on the last fragment → end of AU 1.
        rtp_packet(103, 9000, 96, true, &fu_a(5, false, true, idr_frag_c)),
        // AU 2: a single non-IDR slice, marker=true.
        rtp_packet(104, 12_000, 96, true, &inter_nal),
    ];

    // Run the pipeline: split RTP → feed depacketizer → collect AUs.
    let mut depack = H264Depacketizer::new();
    let mut access_units = Vec::new();
    for packet in &packets {
        let (payload, marker) = split_rtp(packet);
        if let Some(au) = depack.process(payload, marker).expect("depack ok") {
            access_units.push(au);
        }
    }

    assert_eq!(access_units.len(), 2, "expected two access units");

    // ── AU 1: SPS + PPS + reassembled IDR ────────────────────────────
    let au1 = &access_units[0];
    assert!(
        au1.keyframe,
        "AU 1 must be flagged as keyframe (contains IDR)"
    );
    let nal_count_1 = au1
        .annex_b
        .windows(4)
        .filter(|w| *w == [0, 0, 0, 1])
        .count();
    assert_eq!(
        nal_count_1, 3,
        "AU 1 must contain three NALs (SPS, PPS, IDR)"
    );
    // SPS and PPS bodies appear verbatim.
    assert!(au1.annex_b.windows(sps_body.len()).any(|w| w == sps_body));
    assert!(au1.annex_b.windows(pps_body.len()).any(|w| w == pps_body));
    // The reassembled IDR body is the concatenation of the three FU-A fragments.
    let mut reassembled_idr = Vec::new();
    reassembled_idr.extend_from_slice(idr_frag_a);
    reassembled_idr.extend_from_slice(idr_frag_b);
    reassembled_idr.extend_from_slice(idr_frag_c);
    assert!(
        au1.annex_b
            .windows(reassembled_idr.len())
            .any(|w| w == reassembled_idr),
        "reassembled IDR fragments must appear in AU 1"
    );

    // ── AU 2: one non-IDR slice ──────────────────────────────────────
    let au2 = &access_units[1];
    assert!(!au2.keyframe, "AU 2 is non-IDR");
    let nal_count_2 = au2
        .annex_b
        .windows(4)
        .filter(|w| *w == [0, 0, 0, 1])
        .count();
    assert_eq!(nal_count_2, 1);
    assert!(au2
        .annex_b
        .windows(inter_body.len())
        .any(|w| w == inter_body));
}

#[test]
fn fu_a_with_packet_loss_drops_partial_nal_cleanly() {
    // Simulate a missing middle fragment: the depacketizer should accept
    // the start, but the end-without-start subsequence should not
    // produce a corrupt NAL. After reset, normal processing resumes.
    let mut depack = H264Depacketizer::new();
    let start = fu_a(5, true, false, b"only-start-arrived");
    depack.process(&start, false).expect("ok");
    assert!(depack.has_pending_fragment());

    // Caller detects the loss (via SequenceTracker, in the real
    // pipeline) and calls reset().
    depack.reset();
    assert!(!depack.has_pending_fragment());

    // Following stream: a complete single NAL with marker (type=1, non-IDR).
    let mut clean_nal = vec![(3u8 << 5) | 1];
    clean_nal.extend_from_slice(b"clean-recovery");
    let au = depack.process(&clean_nal, true).unwrap().expect("AU");
    assert!(!au.keyframe);
    assert!(
        au.annex_b
            .windows(b"clean-recovery".len())
            .any(|w| w == b"clean-recovery"),
        "recovery NAL must appear in AU"
    );
    // The orphan fragment must NOT appear.
    assert!(
        !au.annex_b
            .windows(b"only-start-arrived".len())
            .any(|w| w == b"only-start-arrived"),
        "orphan fragment must be discarded by reset"
    );
}
