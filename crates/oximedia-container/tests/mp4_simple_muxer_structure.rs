// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Structural validation of [`SimpleMp4Muxer`] output for patent-encumbered
//! codecs.
//!
//! `SimpleMp4Muxer` is the only writer in the workspace that accepts an
//! arbitrary sample-entry fourcc plus opaque extradata, which makes it the only
//! way to emit an H.264 + AAC MP4. Until now nothing exercised it with a real
//! `avcC` or `esds`, and its `mdat` layout was "all of track 0, then all of
//! track 1" with one chunk per sample.
//!
//! **The main `Mp4Demuxer` deliberately refuses `avc1`/`mp4a`** — that patent
//! whitelist is policy and is *not* relaxed for testing. Everything below is
//! therefore validated with a self-contained, test-local ISOBMFF box walker
//! that never touches the production demuxer.

use oximedia_container::mux::mp4::simple::{
    build_audio_specific_config, build_esds_payload, AudioCodecInfo, EsdsParams, Mp4Sample,
    SimpleMp4Config, SimpleMp4Muxer, TrackCodec, VideoCodecInfo,
};
use std::io::Cursor;

// ─── Test-side ISOBMFF box walker ────────────────────────────────────────────

/// One box located inside the buffer.
#[derive(Clone, Debug)]
struct MpBox {
    fourcc: [u8; 4],
    /// Offset of the box header.
    start: usize,
    /// Offset of the first payload byte.
    payload_start: usize,
    /// Offset one past the last payload byte.
    end: usize,
}

impl MpBox {
    fn payload<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        &data[self.payload_start..self.end]
    }

    fn name(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).into_owned()
    }
}

fn be_u16(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn be_u32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn be_u64(data: &[u8], at: usize) -> u64 {
    u64::from_be_bytes([
        data[at],
        data[at + 1],
        data[at + 2],
        data[at + 3],
        data[at + 4],
        data[at + 5],
        data[at + 6],
        data[at + 7],
    ])
}

/// Lists the boxes directly contained in `data[start..end]`.
fn boxes_in(data: &[u8], start: usize, end: usize) -> Vec<MpBox> {
    let mut out = Vec::new();
    let mut offset = start;
    while offset + 8 <= end {
        let size32 = be_u32(data, offset) as u64;
        let mut fourcc = [0u8; 4];
        fourcc.copy_from_slice(&data[offset + 4..offset + 8]);
        let (total, header_len) = if size32 == 1 {
            assert!(offset + 16 <= end, "truncated largesize header");
            (be_u64(data, offset + 8), 16usize)
        } else if size32 == 0 {
            ((end - offset) as u64, 8usize)
        } else {
            (size32, 8usize)
        };
        let total = total as usize;
        assert!(
            total >= header_len && offset + total <= end,
            "box {} at {offset} declares an impossible size {total}",
            String::from_utf8_lossy(&fourcc)
        );
        out.push(MpBox {
            fourcc,
            start: offset,
            payload_start: offset + header_len,
            end: offset + total,
        });
        offset += total;
    }
    assert_eq!(offset, end, "boxes must tile their container exactly");
    out
}

/// Top-level boxes of the file.
fn top_level(data: &[u8]) -> Vec<MpBox> {
    boxes_in(data, 0, data.len())
}

/// Finds a single child by fourcc.
fn child(data: &[u8], parent: &MpBox, fourcc: &[u8; 4]) -> MpBox {
    boxes_in(data, parent.payload_start, parent.end)
        .into_iter()
        .find(|b| &b.fourcc == fourcc)
        .unwrap_or_else(|| {
            panic!(
                "{} has no {} child",
                parent.name(),
                String::from_utf8_lossy(fourcc)
            )
        })
}

/// Walks a chain of nested container boxes starting at `root`.
fn descend(data: &[u8], root: &MpBox, path: &[&[u8; 4]]) -> MpBox {
    let mut current = root.clone();
    for fourcc in path {
        current = child(data, &current, fourcc);
    }
    current
}

fn top(data: &[u8], fourcc: &[u8; 4]) -> MpBox {
    top_level(data)
        .into_iter()
        .find(|b| &b.fourcc == fourcc)
        .unwrap_or_else(|| panic!("no top-level {}", String::from_utf8_lossy(fourcc)))
}

/// The `trak` boxes of the file, in order.
fn traks(data: &[u8]) -> Vec<MpBox> {
    let moov = top(data, b"moov");
    boxes_in(data, moov.payload_start, moov.end)
        .into_iter()
        .filter(|b| &b.fourcc == b"trak")
        .collect()
}

fn stbl_of(data: &[u8], trak: &MpBox) -> MpBox {
    descend(data, trak, &[b"mdia", b"minf", b"stbl"])
}

/// The single sample entry inside a `stsd`, with its fourcc.
fn sample_entry(data: &[u8], stbl: &MpBox) -> MpBox {
    let stsd = child(data, stbl, b"stsd");
    // FullBox: version(1) + flags(3), then entry_count(4).
    let entry_count = be_u32(data, stsd.payload_start + 4);
    assert_eq!(entry_count, 1, "fixture writes exactly one stsd entry");
    let entries = boxes_in(data, stsd.payload_start + 8, stsd.end);
    assert_eq!(entries.len(), 1);
    entries[0].clone()
}

/// Child boxes of a *visual* sample entry (after its 78-byte fixed part).
fn visual_entry_children(data: &[u8], entry: &MpBox) -> Vec<MpBox> {
    boxes_in(data, entry.payload_start + 78, entry.end)
}

/// Child boxes of an *audio* sample entry (after its 28-byte fixed part).
fn audio_entry_children(data: &[u8], entry: &MpBox) -> Vec<MpBox> {
    boxes_in(data, entry.payload_start + 28, entry.end)
}

// ─── Parsed sample tables ────────────────────────────────────────────────────

#[derive(Debug)]
struct SampleTable {
    /// `(count, delta)` from `stts`.
    stts: Vec<(u32, u32)>,
    /// `(count, offset)` from `ctts`, empty when absent.
    ctts: Vec<(u32, i32)>,
    /// `(first_chunk, samples_per_chunk, description_index)` from `stsc`.
    stsc: Vec<(u32, u32, u32)>,
    /// Per-sample sizes expanded from `stsz`.
    sizes: Vec<u32>,
    /// Chunk offsets from `stco` or `co64`.
    chunk_offsets: Vec<u64>,
    /// `true` when the offsets came from a `co64` box.
    used_co64: bool,
    /// 1-based sync sample numbers from `stss`, `None` when absent.
    stss: Option<Vec<u32>>,
}

fn parse_sample_table(data: &[u8], stbl: &MpBox) -> SampleTable {
    let children = boxes_in(data, stbl.payload_start, stbl.end);
    let find = |fourcc: &[u8; 4]| children.iter().find(|b| &b.fourcc == fourcc).cloned();

    let stts_box = find(b"stts").expect("stts is mandatory");
    let stts_payload = stts_box.payload(data);
    let mut stts = Vec::new();
    for i in 0..be_u32(stts_payload, 4) as usize {
        let at = 8 + i * 8;
        stts.push((be_u32(stts_payload, at), be_u32(stts_payload, at + 4)));
    }

    let mut ctts = Vec::new();
    if let Some(ctts_box) = find(b"ctts") {
        let p = ctts_box.payload(data);
        for i in 0..be_u32(p, 4) as usize {
            let at = 8 + i * 8;
            #[allow(clippy::cast_possible_wrap)]
            ctts.push((be_u32(p, at), be_u32(p, at + 4) as i32));
        }
    }

    let stsc_box = find(b"stsc").expect("stsc is mandatory");
    let p = stsc_box.payload(data);
    let mut stsc = Vec::new();
    for i in 0..be_u32(p, 4) as usize {
        let at = 8 + i * 12;
        stsc.push((be_u32(p, at), be_u32(p, at + 4), be_u32(p, at + 8)));
    }

    let stsz_box = find(b"stsz").expect("stsz is mandatory");
    let p = stsz_box.payload(data);
    let uniform = be_u32(p, 4);
    let sample_count = be_u32(p, 8) as usize;
    let sizes = if uniform > 0 {
        vec![uniform; sample_count]
    } else {
        (0..sample_count).map(|i| be_u32(p, 12 + i * 4)).collect()
    };

    let (chunk_offsets, used_co64) = if let Some(co64) = find(b"co64") {
        let p = co64.payload(data);
        let count = be_u32(p, 4) as usize;
        ((0..count).map(|i| be_u64(p, 8 + i * 8)).collect(), true)
    } else {
        let stco = find(b"stco").expect("stco or co64 is mandatory");
        let p = stco.payload(data);
        let count = be_u32(p, 4) as usize;
        (
            (0..count)
                .map(|i| u64::from(be_u32(p, 8 + i * 4)))
                .collect(),
            false,
        )
    };

    let stss = find(b"stss").map(|b| {
        let p = b.payload(data);
        (0..be_u32(p, 4) as usize)
            .map(|i| be_u32(p, 8 + i * 4))
            .collect()
    });

    SampleTable {
        stts,
        ctts,
        stsc,
        sizes,
        chunk_offsets,
        used_co64,
        stss,
    }
}

impl SampleTable {
    fn sample_count(&self) -> usize {
        self.sizes.len()
    }

    /// Expands `stsc` into a per-chunk sample count.
    fn samples_per_chunk(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.chunk_offsets.len());
        for chunk in 1..=self.chunk_offsets.len() as u32 {
            let entry = self
                .stsc
                .iter()
                .rev()
                .find(|(first, _, _)| *first <= chunk)
                .unwrap_or_else(|| panic!("stsc has no entry covering chunk {chunk}"));
            out.push(entry.1);
        }
        out
    }

    /// Rebuilds every sample's absolute file offset from `stsc`/`stco`/`stsz`.
    fn sample_offsets(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.sample_count());
        let mut sample = 0usize;
        for (chunk_index, &chunk_offset) in self.chunk_offsets.iter().enumerate() {
            let count = self.samples_per_chunk()[chunk_index];
            let mut cursor = chunk_offset;
            for _ in 0..count {
                out.push(cursor);
                cursor += u64::from(self.sizes[sample]);
                sample += 1;
            }
        }
        assert_eq!(
            sample,
            self.sample_count(),
            "stsc chunks must cover every stsz sample exactly once"
        );
        out
    }

    fn durations(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for &(count, delta) in &self.stts {
            for _ in 0..count {
                out.push(delta);
            }
        }
        out
    }

    fn composition_offsets(&self) -> Vec<i32> {
        let mut out = Vec::new();
        for &(count, offset) in &self.ctts {
            for _ in 0..count {
                out.push(offset);
            }
        }
        out
    }
}

// ─── Fixture: H.264-shaped video + AAC-shaped audio ──────────────────────────

const VIDEO_TIMESCALE: u32 = 90_000;
const VIDEO_FRAME_DURATION: u32 = 3_000;
const AUDIO_SAMPLE_RATE: u32 = 44_100;
const AAC_FRAME_SAMPLES: u32 = 1024;
const NUM_VIDEO_SAMPLES: usize = 12;
const NUM_AUDIO_SAMPLES: usize = 20;

/// Canned SPS for 640x360 Baseline (profile 0x42, level 0x1E).
const SPS: [u8; 12] = [
    0x67, 0x42, 0xC0, 0x1E, 0xD9, 0x00, 0xA0, 0x2F, 0xF9, 0x70, 0x11, 0x00,
];
/// Canned PPS.
const PPS: [u8; 4] = [0x68, 0xCE, 0x3C, 0x80];

/// Builds an `avcC` (AVCDecoderConfigurationRecord) payload from SPS/PPS.
fn build_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut c = Vec::new();
    c.push(0x01); // configurationVersion
    c.push(sps[1]); // AVCProfileIndication
    c.push(sps[2]); // profile_compatibility
    c.push(sps[3]); // AVCLevelIndication
    c.push(0xFF); // 111111 reserved + lengthSizeMinusOne = 3 (4-byte lengths)
    c.push(0xE1); // 111 reserved + numOfSequenceParameterSets = 1
    c.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    c.extend_from_slice(sps);
    c.push(0x01); // numOfPictureParameterSets
    c.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    c.extend_from_slice(pps);
    c
}

/// Builds an AVCC-framed access unit: 4-byte length prefix + NAL bytes.
fn avcc_access_unit(frame: usize, keyframe: bool) -> Vec<u8> {
    let nal_type: u8 = if keyframe { 0x65 } else { 0x41 };
    let body_len = 16 + frame % 5;
    let mut nal = Vec::with_capacity(1 + body_len);
    nal.push(nal_type);
    for i in 0..body_len {
        nal.push(((frame * 31 + i * 7) & 0xFF) as u8);
    }

    let mut out = Vec::with_capacity(4 + nal.len());
    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
    out.extend_from_slice(&nal);
    out
}

/// A dummy AAC frame body (an ADTS-less raw AAC payload would go here).
fn aac_frame(index: usize) -> Vec<u8> {
    let len = 96 + index % 9;
    (0..len)
        .map(|i| ((index * 13 + i * 3) & 0xFF) as u8)
        .collect()
}

/// `(pts, dts, is_sync)` for a display order with a classic IBBP pattern:
/// every 4th frame is an I-frame, and P/B frames swap presentation order so a
/// `ctts` table is required.
fn video_timing(frame: usize) -> (i64, i64, bool) {
    let dts = (frame as i64) * i64::from(VIDEO_FRAME_DURATION);
    // Reorder offset cycles 0, +2, -1, -1 frames.
    let offset_frames = match frame % 4 {
        0 => 0i64,
        1 => 2,
        _ => -1,
    };
    let pts = dts + offset_frames * i64::from(VIDEO_FRAME_DURATION);
    (pts, dts, frame % 4 == 0)
}

struct Fixture {
    data: Vec<u8>,
    video_samples: Vec<Vec<u8>>,
    audio_samples: Vec<Vec<u8>>,
    avcc: Vec<u8>,
    esds: Vec<u8>,
}

fn build_fixture() -> Fixture {
    let avcc = build_avcc(&SPS, &PPS);
    let asc = build_audio_specific_config(2, AUDIO_SAMPLE_RATE, 2);
    let esds = build_esds_payload(
        &asc,
        &EsdsParams {
            es_id: 2,
            buffer_size_db: 6144,
            max_bitrate: 128_000,
            avg_bitrate: 128_000,
        },
    );

    let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::default());
    let video_track = muxer.add_track(TrackCodec::Video(
        VideoCodecInfo::new(*b"avc1", 640, 360, VIDEO_TIMESCALE)
            .with_config(*b"avcC", avcc.clone()),
    ));
    let audio_track = muxer.add_track(TrackCodec::Audio(
        AudioCodecInfo::new(*b"mp4a", AUDIO_SAMPLE_RATE, 2).with_config(*b"esds", esds.clone()),
    ));

    let mut video_samples = Vec::new();
    for frame in 0..NUM_VIDEO_SAMPLES {
        let (pts, dts, is_sync) = video_timing(frame);
        let data = avcc_access_unit(frame, is_sync);
        video_samples.push(data.clone());
        muxer
            .write_sample(
                video_track,
                Mp4Sample {
                    pts,
                    dts,
                    duration: VIDEO_FRAME_DURATION,
                    is_sync,
                    data,
                },
            )
            .expect("write video sample");
    }

    let mut audio_samples = Vec::new();
    for index in 0..NUM_AUDIO_SAMPLES {
        let data = aac_frame(index);
        audio_samples.push(data.clone());
        let dts = (index as i64) * i64::from(AAC_FRAME_SAMPLES);
        muxer
            .write_sample(
                audio_track,
                Mp4Sample {
                    pts: dts,
                    dts,
                    duration: AAC_FRAME_SAMPLES,
                    is_sync: true,
                    data,
                },
            )
            .expect("write audio sample");
    }

    let mut out = Cursor::new(Vec::<u8>::new());
    muxer.finalize(&mut out).expect("finalize");

    Fixture {
        data: out.into_inner(),
        video_samples,
        audio_samples,
        avcc,
        esds,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn file_layout_is_ftyp_moov_mdat() {
    let fixture = build_fixture();
    let boxes = top_level(&fixture.data);
    let names: Vec<String> = boxes.iter().map(MpBox::name).collect();
    assert_eq!(
        names,
        vec!["ftyp".to_string(), "moov".to_string(), "mdat".to_string()],
        "progressive output must be exactly ftyp + moov + mdat"
    );
    assert_eq!(boxes[0].start, 0, "ftyp must be first");
    assert_eq!(
        &fixture.data[boxes[0].payload_start..boxes[0].payload_start + 4],
        b"isom",
        "default major brand"
    );
}

#[test]
fn moov_has_one_trak_per_track() {
    let fixture = build_fixture();
    assert_eq!(traks(&fixture.data).len(), 2, "video + audio");
    let moov = top(&fixture.data, b"moov");
    // mvhd must precede the traks.
    let children = boxes_in(&fixture.data, moov.payload_start, moov.end);
    assert_eq!(children[0].fourcc, *b"mvhd");
}

#[test]
fn stsd_entries_carry_the_requested_fourccs() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);

    let video_entry = sample_entry(&fixture.data, &stbl_of(&fixture.data, &tracks[0]));
    assert_eq!(&video_entry.fourcc, b"avc1");
    let audio_entry = sample_entry(&fixture.data, &stbl_of(&fixture.data, &tracks[1]));
    assert_eq!(&audio_entry.fourcc, b"mp4a");

    // Visual sample entry fixed fields.
    let p = video_entry.payload(&fixture.data);
    assert_eq!(be_u16(p, 6), 1, "data_reference_index");
    assert_eq!(be_u16(p, 24), 640, "width");
    assert_eq!(be_u16(p, 26), 360, "height");

    // Audio sample entry fixed fields.
    let p = audio_entry.payload(&fixture.data);
    assert_eq!(be_u16(p, 6), 1, "data_reference_index");
    assert_eq!(be_u16(p, 16), 2, "channel_count");
    assert_eq!(be_u32(p, 24) >> 16, AUDIO_SAMPLE_RATE, "sample rate 16.16");
}

#[test]
fn avcc_payload_round_trips() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);
    let entry = sample_entry(&fixture.data, &stbl_of(&fixture.data, &tracks[0]));
    let children = visual_entry_children(&fixture.data, &entry);
    assert_eq!(children.len(), 1, "exactly one config box");
    assert_eq!(&children[0].fourcc, b"avcC");

    let payload = children[0].payload(&fixture.data);
    assert_eq!(payload, fixture.avcc.as_slice(), "avcC bytes must survive");

    // And it must still decode as an AVCDecoderConfigurationRecord.
    assert_eq!(payload[0], 1, "configurationVersion");
    assert_eq!(payload[1], SPS[1], "AVCProfileIndication");
    assert_eq!(payload[3], SPS[3], "AVCLevelIndication");
    assert_eq!(payload[4] & 0x03, 3, "lengthSizeMinusOne = 3");
    let num_sps = usize::from(payload[5] & 0x1F);
    assert_eq!(num_sps, 1);
    let sps_len = usize::from(be_u16(payload, 6));
    assert_eq!(&payload[8..8 + sps_len], &SPS[..]);
    let pps_base = 8 + sps_len;
    assert_eq!(payload[pps_base], 1, "numOfPictureParameterSets");
    let pps_len = usize::from(be_u16(payload, pps_base + 1));
    assert_eq!(&payload[pps_base + 3..pps_base + 3 + pps_len], &PPS[..]);
}

#[test]
fn esds_payload_round_trips_and_still_decodes() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);
    let entry = sample_entry(&fixture.data, &stbl_of(&fixture.data, &tracks[1]));
    let children = audio_entry_children(&fixture.data, &entry);
    assert_eq!(children.len(), 1);
    assert_eq!(&children[0].fourcc, b"esds");

    let payload = children[0].payload(&fixture.data);
    assert_eq!(payload, fixture.esds.as_slice(), "esds bytes must survive");

    // Decode the descriptor chain: ES → DecoderConfig → DecoderSpecificInfo.
    assert_eq!(&payload[0..4], &[0, 0, 0, 0], "FullBox version + flags");
    assert_eq!(payload[4], 0x03, "ES_Descriptor tag");
    let es = descriptor_body(&payload[4..]);
    assert_eq!(be_u16(es, 0), 2, "ES_ID");
    assert_eq!(es[3], 0x04, "DecoderConfigDescriptor tag");
    let dcd = descriptor_body(&es[3..]);
    assert_eq!(dcd[0], 0x40, "objectTypeIndication = MPEG-4 Audio");
    assert_eq!(dcd[1] >> 2, 0x05, "streamType = AudioStream");
    assert_eq!(dcd[13], 0x05, "DecoderSpecificInfo tag");
    let asc = descriptor_body(&dcd[13..]);
    assert_eq!(
        asc,
        build_audio_specific_config(2, AUDIO_SAMPLE_RATE, 2).as_slice(),
        "AudioSpecificConfig must survive the descriptor round-trip"
    );
}

/// Reads the body of the MPEG-4 descriptor starting at `data[0]`.
fn descriptor_body(data: &[u8]) -> &[u8] {
    let mut length = 0usize;
    let mut used = 0usize;
    loop {
        let byte = data[1 + used];
        length = (length << 7) | usize::from(byte & 0x7F);
        used += 1;
        if byte & 0x80 == 0 || used == 4 {
            break;
        }
    }
    &data[1 + used..1 + used + length]
}

#[test]
fn sample_tables_are_internally_consistent() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);

    let video = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[0]));
    let audio = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[1]));

    assert_eq!(video.sample_count(), NUM_VIDEO_SAMPLES);
    assert_eq!(audio.sample_count(), NUM_AUDIO_SAMPLES);

    // stts covers every sample.
    assert_eq!(video.durations().len(), NUM_VIDEO_SAMPLES);
    assert!(video.durations().iter().all(|&d| d == VIDEO_FRAME_DURATION));
    assert_eq!(audio.durations().len(), NUM_AUDIO_SAMPLES);
    assert!(audio.durations().iter().all(|&d| d == AAC_FRAME_SAMPLES));

    // stsz sizes match the sample payloads.
    for (index, size) in video.sizes.iter().enumerate() {
        assert_eq!(*size as usize, fixture.video_samples[index].len());
    }
    for (index, size) in audio.sizes.iter().enumerate() {
        assert_eq!(*size as usize, fixture.audio_samples[index].len());
    }

    // stsc must account for every chunk, and the chunk counts must sum to the
    // sample count.
    for table in [&video, &audio] {
        assert!(!table.chunk_offsets.is_empty(), "at least one chunk");
        assert_eq!(table.stsc[0].0, 1, "stsc must start at chunk 1");
        assert!(
            table.stsc.iter().all(|(_, _, desc)| *desc == 1),
            "single stsd entry → description index 1"
        );
        let per_chunk = table.samples_per_chunk();
        assert_eq!(per_chunk.len(), table.chunk_offsets.len());
        let total: u32 = per_chunk.iter().sum();
        assert_eq!(total as usize, table.sample_count());
        assert!(
            table.chunk_offsets.windows(2).all(|w| w[0] < w[1]),
            "chunk offsets must strictly increase"
        );
        assert!(!table.used_co64, "a small fixture must use 32-bit stco");
    }
}

#[test]
fn ctts_matches_the_written_reorder_pattern() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);
    let video = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[0]));
    let audio = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[1]));

    let offsets = video.composition_offsets();
    assert_eq!(offsets.len(), NUM_VIDEO_SAMPLES, "ctts must cover video");
    for frame in 0..NUM_VIDEO_SAMPLES {
        let (pts, dts, _) = video_timing(frame);
        assert_eq!(
            i64::from(offsets[frame]),
            pts - dts,
            "ctts offset for frame {frame}"
        );
    }
    assert!(
        offsets.iter().any(|&o| o < 0),
        "the fixture exercises negative composition offsets"
    );

    assert!(
        audio.ctts.is_empty(),
        "audio has pts == dts, so no ctts should be written"
    );
}

#[test]
fn stss_lists_exactly_the_sync_samples() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);
    let video = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[0]));
    let audio = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[1]));

    let expected: Vec<u32> = (0..NUM_VIDEO_SAMPLES)
        .filter(|&f| video_timing(f).2)
        .map(|f| (f + 1) as u32)
        .collect();
    assert_eq!(video.stss.as_ref(), Some(&expected));

    assert!(
        audio.stss.is_none(),
        "an all-sync audio track needs no stss"
    );
}

/// The strongest check: rebuild every sample's byte range from the tables and
/// compare it against the exact bytes handed to the muxer.
#[test]
fn every_sample_is_addressable_and_byte_exact() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);

    for (track_index, expected) in [
        (0usize, &fixture.video_samples),
        (1usize, &fixture.audio_samples),
    ] {
        let table =
            parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[track_index]));
        let offsets = table.sample_offsets();
        assert_eq!(offsets.len(), expected.len());
        for (index, (&offset, want)) in offsets.iter().zip(expected.iter()).enumerate() {
            let start = usize::try_from(offset).expect("offset fits usize");
            let end = start + want.len();
            assert!(
                end <= fixture.data.len(),
                "track {track_index} sample {index}: offset {start}..{end} is past EOF"
            );
            assert_eq!(
                &fixture.data[start..end],
                want.as_slice(),
                "track {track_index} sample {index}: bytes at the table offset"
            );
        }
    }
}

/// The whole point of the interleaver: samples must be ordered by DTS in
/// `mdat`, not "all of track 0, then all of track 1".
#[test]
fn mdat_is_interleaved_by_decode_time() {
    let fixture = build_fixture();
    let tracks = traks(&fixture.data);
    let video = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[0]));
    let audio = parse_sample_table(&fixture.data, &stbl_of(&fixture.data, &tracks[1]));

    // Both tracks must own more than one chunk — a single chunk each is exactly
    // the old "write track 0 then track 1" layout.
    assert!(
        video.chunk_offsets.len() > 1,
        "video must be split across several chunks; got {}",
        video.chunk_offsets.len()
    );
    assert!(
        audio.chunk_offsets.len() > 1,
        "audio must be split across several chunks; got {}",
        audio.chunk_offsets.len()
    );

    // Merge all chunks by file offset and check the tracks alternate.
    let mut chunks: Vec<(u64, usize)> = Vec::new();
    for &offset in &video.chunk_offsets {
        chunks.push((offset, 0));
    }
    for &offset in &audio.chunk_offsets {
        chunks.push((offset, 1));
    }
    chunks.sort_unstable();
    let alternations = chunks.windows(2).filter(|w| w[0].1 != w[1].1).count();
    assert!(
        alternations >= 4,
        "mdat must interleave the two tracks repeatedly; only {alternations} switches"
    );

    // Every sample, in mdat order, must be non-decreasing in wall-clock DTS.
    let mut timeline: Vec<(f64, usize)> = Vec::new();
    for (index, &offset) in video.sample_offsets().iter().enumerate() {
        let dts = video_timing(index).1 as f64 / f64::from(VIDEO_TIMESCALE);
        timeline.push((dts, usize::try_from(offset).expect("offset")));
    }
    for index in 0..NUM_AUDIO_SAMPLES {
        let offset = audio.sample_offsets()[index];
        let dts = (index as f64 * f64::from(AAC_FRAME_SAMPLES)) / f64::from(AUDIO_SAMPLE_RATE);
        timeline.push((dts, usize::try_from(offset).expect("offset")));
    }
    timeline.sort_by(|a, b| a.1.cmp(&b.1)); // sort by file position

    // Allow a small slack: samples inside one chunk share a position range, and
    // a chunk boundary can straddle at most one frame duration.
    let slack = f64::from(VIDEO_FRAME_DURATION) / f64::from(VIDEO_TIMESCALE);
    for window in timeline.windows(2) {
        assert!(
            window[1].0 + slack >= window[0].0,
            "mdat order regressed in time: {:?} then {:?}",
            window[0],
            window[1]
        );
    }
}

/// A fragmented `SimpleMp4Muxer` file must carry a spec-shaped init segment.
#[test]
fn fragmented_init_segment_has_empty_sample_tables() {
    let avcc = build_avcc(&SPS, &PPS);
    let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config {
        fragmented: true,
        ..Default::default()
    });
    let track = muxer.add_track(TrackCodec::Video(
        VideoCodecInfo::new(*b"avc1", 640, 360, VIDEO_TIMESCALE).with_config(*b"avcC", avcc),
    ));
    for frame in 0..4usize {
        let (pts, dts, is_sync) = video_timing(frame);
        muxer
            .write_sample(
                track,
                Mp4Sample {
                    pts,
                    dts,
                    duration: VIDEO_FRAME_DURATION,
                    is_sync,
                    data: avcc_access_unit(frame, is_sync),
                },
            )
            .expect("write sample");
    }
    let mut out = Cursor::new(Vec::<u8>::new());
    muxer.finalize(&mut out).expect("finalize");
    let data = out.into_inner();

    let names: Vec<String> = top_level(&data).iter().map(MpBox::name).collect();
    assert_eq!(
        names,
        vec![
            "ftyp".to_string(),
            "moov".to_string(),
            "moof".to_string(),
            "mdat".to_string()
        ]
    );

    let moov = top(&data, b"moov");
    let mvex = child(&data, &moov, b"mvex");
    assert_eq!(child(&data, &mvex, b"trex").fourcc, *b"trex");

    let table = parse_sample_table(&data, &stbl_of(&data, &traks(&data)[0]));
    assert_eq!(table.sample_count(), 0, "init segment stsz must be empty");
    assert!(table.stts.is_empty(), "init segment stts must be empty");
    assert!(table.stsc.is_empty(), "init segment stsc must be empty");
    assert!(
        table.chunk_offsets.is_empty(),
        "init segment stco must be empty"
    );

    // The stsd entry (and its avcC) must still be there.
    let entry = sample_entry(&data, &stbl_of(&data, &traks(&data)[0]));
    assert_eq!(&entry.fourcc, b"avc1");
    assert_eq!(
        visual_entry_children(&data, &entry)[0].fourcc,
        *b"avcC",
        "init segment keeps the codec config"
    );
}

/// `trun.data_offset` used to be written as a never-patched `0`, pointing every
/// sample read at the middle of the `moof` header.
#[test]
fn fragment_trun_data_offset_points_at_the_mdat_payload() {
    let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config {
        fragmented: true,
        ..Default::default()
    });
    let track = muxer.add_track(TrackCodec::Video(VideoCodecInfo::new(
        *b"avc1",
        640,
        360,
        VIDEO_TIMESCALE,
    )));
    let mut written = Vec::new();
    for frame in 0..3usize {
        let (pts, dts, is_sync) = video_timing(frame);
        let sample = avcc_access_unit(frame, is_sync);
        written.push(sample.clone());
        muxer
            .write_sample(
                track,
                Mp4Sample {
                    pts,
                    dts,
                    duration: VIDEO_FRAME_DURATION,
                    is_sync,
                    data: sample,
                },
            )
            .expect("write sample");
    }
    let mut out = Cursor::new(Vec::<u8>::new());
    muxer.finalize(&mut out).expect("finalize");
    let data = out.into_inner();

    let moof = top(&data, b"moof");
    let mdat = top(&data, b"mdat");
    let traf = child(&data, &moof, b"traf");
    let trun = child(&data, &traf, b"trun");

    let p = trun.payload(&data);
    let version = p[0];
    let flags = u32::from(p[1]) << 16 | u32::from(p[2]) << 8 | u32::from(p[3]);
    assert_eq!(version, 1, "version 1 → signed composition offsets");
    assert_eq!(
        flags & 0x0F01,
        0x0F01,
        "data-offset + duration + size + flags + CTO must all be declared"
    );

    let sample_count = be_u32(p, 4) as usize;
    assert_eq!(sample_count, written.len());
    #[allow(clippy::cast_possible_wrap)]
    let data_offset = be_u32(p, 8) as i32;

    // tfhd sets default-base-is-moof, so the offset is relative to moof.start.
    let tfhd = child(&data, &traf, b"tfhd");
    let tfhd_flags =
        u32::from(tfhd.payload(&data)[1]) << 16 | u32::from(tfhd.payload(&data)[2]) << 8;
    assert_eq!(
        tfhd_flags & 0x0002_0000,
        0x0002_0000,
        "default-base-is-moof"
    );

    let mut cursor = moof.start + usize::try_from(data_offset).expect("non-negative data offset");
    assert_eq!(
        cursor, mdat.payload_start,
        "data_offset must land exactly on the mdat payload"
    );

    // Walk the run and verify each sample's bytes.
    for (index, want) in written.iter().enumerate() {
        let at = 12 + index * 16;
        let duration = be_u32(p, at);
        let size = be_u32(p, at + 4) as usize;
        let sample_flags = be_u32(p, at + 8);
        assert_eq!(duration, VIDEO_FRAME_DURATION);
        assert_eq!(size, want.len(), "sample {index} size");
        let is_sync = sample_flags & 0x0001_0000 == 0;
        assert_eq!(is_sync, video_timing(index).2, "sample {index} sync flag");
        assert_eq!(
            &data[cursor..cursor + size],
            want.as_slice(),
            "sample {index} bytes"
        );
        cursor += size;
    }
    assert_eq!(cursor, mdat.end, "the run must cover the whole mdat");
}
