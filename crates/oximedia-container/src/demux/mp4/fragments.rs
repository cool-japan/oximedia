//! Fragmented MP4 (`moof`) parsing for the [`Mp4Demuxer`](super::Mp4Demuxer).
//!
//! A fragmented MP4 (fMP4/CMAF) file has an *init segment* — `ftyp` plus a
//! `moov` whose sample tables are empty and which carries an `mvex` box with
//! one `trex` per track — followed by any number of *media segments*, each a
//! `moof` box (describing the samples) paired with an `mdat` box (holding the
//! bytes).
//!
//! This module turns those `moof` boxes into flat per-sample descriptions that
//! the demuxer can splice onto its regular sample tables, so a fragmented file
//! reads back exactly like the progressive file built from the same packets.
//!
//! Boxes handled here:
//!
//! | Box    | Purpose                                                            |
//! |--------|--------------------------------------------------------------------|
//! | `mvex` | Container for `trex` (parsed while parsing `moov`)                 |
//! | `trex` | Per-track fragment defaults (duration/size/flags)                  |
//! | `moof` | Movie fragment container                                           |
//! | `mfhd` | Fragment sequence number                                           |
//! | `traf` | Per-track fragment container                                       |
//! | `tfhd` | Track fragment header: base data offset + per-fragment defaults    |
//! | `tfdt` | Base media decode time for the fragment                            |
//! | `trun` | Track fragment run: per-sample duration/size/flags/composition time |

use std::collections::HashMap;

use oximedia_core::{OxiError, OxiResult};

use super::atom::Mp4Atom;
use super::boxes::{BoxHeader, BoxType};

// ─── tfhd flags (ISO/IEC 14496-12 §8.8.7) ────────────────────────────────────

/// `tfhd`: an explicit 64-bit `base_data_offset` follows.
const TFHD_BASE_DATA_OFFSET_PRESENT: u32 = 0x0000_0001;
/// `tfhd`: an explicit `sample_description_index` follows.
const TFHD_SAMPLE_DESCRIPTION_INDEX_PRESENT: u32 = 0x0000_0002;
/// `tfhd`: an explicit `default_sample_duration` follows.
const TFHD_DEFAULT_SAMPLE_DURATION_PRESENT: u32 = 0x0000_0008;
/// `tfhd`: an explicit `default_sample_size` follows.
const TFHD_DEFAULT_SAMPLE_SIZE_PRESENT: u32 = 0x0000_0010;
/// `tfhd`: an explicit `default_sample_flags` follows.
const TFHD_DEFAULT_SAMPLE_FLAGS_PRESENT: u32 = 0x0000_0020;
/// `tfhd`: sample offsets are relative to the start of the enclosing `moof`.
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x0002_0000;

// ─── trun flags (ISO/IEC 14496-12 §8.8.8) ────────────────────────────────────

/// `trun`: a signed 32-bit `data_offset` follows the sample count.
const TRUN_DATA_OFFSET_PRESENT: u32 = 0x0000_0001;
/// `trun`: an explicit flags word for the first sample follows.
const TRUN_FIRST_SAMPLE_FLAGS_PRESENT: u32 = 0x0000_0004;
/// `trun`: each sample record carries its own duration.
const TRUN_SAMPLE_DURATION_PRESENT: u32 = 0x0000_0100;
/// `trun`: each sample record carries its own size.
const TRUN_SAMPLE_SIZE_PRESENT: u32 = 0x0000_0200;
/// `trun`: each sample record carries its own flags.
const TRUN_SAMPLE_FLAGS_PRESENT: u32 = 0x0000_0400;
/// `trun`: each sample record carries a composition time offset.
const TRUN_SAMPLE_CTS_PRESENT: u32 = 0x0000_0800;

/// Bit 16 of a sample-flags word: `sample_is_non_sync_sample`.
const SAMPLE_IS_NON_SYNC: u32 = 0x0001_0000;

/// Hard cap on the number of samples one `trun` may declare.
///
/// The 32-bit `sample_count` is attacker controlled. Even with the
/// bytes-remaining check below, a `trun` whose per-sample record size is zero
/// (every optional field absent) would otherwise let a 16-byte box declare
/// `u32::MAX` samples and drive an unbounded `Vec` growth. Legitimate runs are
/// orders of magnitude smaller than this bound.
const MAX_TRUN_SAMPLE_COUNT: usize = 4_000_000;

// ─── Public types ────────────────────────────────────────────────────────────

/// A `trex` (track extends) box: per-track defaults for every fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrexBox {
    /// Track ID these defaults apply to.
    pub track_id: u32,
    /// Default `stsd` entry index (1-based).
    pub default_sample_description_index: u32,
    /// Default sample duration in media timescale units.
    pub default_sample_duration: u32,
    /// Default sample size in bytes.
    pub default_sample_size: u32,
    /// Default sample flags word.
    pub default_sample_flags: u32,
}

/// One sample described by a `trun` inside a `moof`.
///
/// Offsets are absolute file positions, ready to be read directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentSample {
    /// Track ID this sample belongs to.
    pub track_id: u32,
    /// Absolute byte offset of the sample payload in the file.
    pub offset: u64,
    /// Sample size in bytes.
    pub size: u32,
    /// Sample duration in media timescale units.
    pub duration: u32,
    /// Composition time offset (PTS − DTS) in media timescale units.
    pub cts_offset: i32,
    /// Absolute decode timestamp in media timescale units.
    pub dts: u64,
    /// Whether this sample is a sync point (keyframe).
    pub is_sync: bool,
}

/// The result of parsing one `moof` box.
#[derive(Clone, Debug, Default)]
pub struct MoofInfo {
    /// `mfhd.sequence_number`, or 0 when no `mfhd` was present.
    pub sequence_number: u32,
    /// Every sample described by the fragment, in file order.
    pub samples: Vec<FragmentSample>,
}

// ─── mvex / trex ─────────────────────────────────────────────────────────────

/// Parses the content of an `mvex` box, returning every `trex` entry.
///
/// Unknown children (`mehd`, `leva`, …) are skipped.
///
/// # Errors
///
/// Returns an error if a `trex` box is truncated.
pub fn parse_mvex(data: &[u8]) -> OxiResult<Vec<TrexBox>> {
    let mut entries = Vec::new();
    for (box_type, content) in ChildBoxes::new(data) {
        if box_type == BoxType::TREX {
            entries.push(parse_trex(content)?);
        }
    }
    Ok(entries)
}

/// Parses the content of a single `trex` box.
fn parse_trex(data: &[u8]) -> OxiResult<TrexBox> {
    let mut atom = Mp4Atom::new(data);
    atom.skip(4)?; // version + flags
    Ok(TrexBox {
        track_id: atom.read_u32()?,
        default_sample_description_index: atom.read_u32()?,
        default_sample_duration: atom.read_u32()?,
        default_sample_size: atom.read_u32()?,
        default_sample_flags: atom.read_u32()?,
    })
}

// ─── moof ────────────────────────────────────────────────────────────────────

/// Parses one `moof` box into a flat list of samples with absolute offsets.
///
/// # Arguments
///
/// * `data` - `moof` box *content* (i.e. after its 8/16-byte header).
/// * `moof_offset` - Absolute file offset of the first byte of the `moof` box
///   header. This is the anchor for `default-base-is-moof` addressing.
/// * `trex` - `trex` defaults harvested from `moov`/`mvex`.
/// * `decode_times` - Running per-track decode time, keyed by track ID. Used
///   when a `traf` omits `tfdt`, and updated as fragments are consumed so
///   consecutive fragments chain correctly.
///
/// # Errors
///
/// Returns an error if a mandatory child box is truncated, if a `trun`
/// declares more samples than its payload can hold, or if a computed byte
/// offset overflows.
pub fn parse_moof(
    data: &[u8],
    moof_offset: u64,
    trex: &[TrexBox],
    decode_times: &mut HashMap<u32, u64>,
) -> OxiResult<MoofInfo> {
    let mut info = MoofInfo::default();
    // ISO/IEC 14496-12 §8.8.7: when `base-data-offset-present` is absent and
    // `default-base-is-moof` is not set, the second and later track fragments
    // continue where the previous one's data ended.
    let mut previous_traf_end: Option<u64> = None;

    for (box_type, content) in ChildBoxes::new(data) {
        match box_type {
            BoxType::MFHD => {
                let mut atom = Mp4Atom::new(content);
                atom.skip(4)?; // version + flags
                info.sequence_number = atom.read_u32()?;
            }
            BoxType::TRAF => {
                parse_traf(
                    content,
                    moof_offset,
                    trex,
                    decode_times,
                    &mut previous_traf_end,
                    &mut info.samples,
                )?;
            }
            _ => {}
        }
    }

    Ok(info)
}

/// Per-track defaults resolved from `tfhd` + `trex`.
#[derive(Clone, Copy, Debug, Default)]
struct TrafDefaults {
    duration: u32,
    size: u32,
    flags: u32,
}

/// Parsed `tfhd` fields.
#[derive(Clone, Copy, Debug, Default)]
struct TfhdBox {
    track_id: u32,
    base_data_offset: Option<u64>,
    default_base_is_moof: bool,
    defaults: TrafDefaults,
}

/// Parses a `tfhd` box, folding in the matching `trex` defaults.
fn parse_tfhd(data: &[u8], trex: &[TrexBox]) -> OxiResult<TfhdBox> {
    let mut atom = Mp4Atom::new(data);
    atom.skip(1)?; // version
    let flags = u32::from(atom.read_u8()?) << 16
        | u32::from(atom.read_u8()?) << 8
        | u32::from(atom.read_u8()?);
    let track_id = atom.read_u32()?;

    let base_data_offset = if flags & TFHD_BASE_DATA_OFFSET_PRESENT != 0 {
        Some(atom.read_u64()?)
    } else {
        None
    };
    if flags & TFHD_SAMPLE_DESCRIPTION_INDEX_PRESENT != 0 {
        atom.skip(4)?;
    }

    let trex_entry = trex.iter().find(|t| t.track_id == track_id).copied();
    let duration = if flags & TFHD_DEFAULT_SAMPLE_DURATION_PRESENT != 0 {
        atom.read_u32()?
    } else {
        trex_entry.map_or(0, |t| t.default_sample_duration)
    };
    let size = if flags & TFHD_DEFAULT_SAMPLE_SIZE_PRESENT != 0 {
        atom.read_u32()?
    } else {
        trex_entry.map_or(0, |t| t.default_sample_size)
    };
    let sample_flags = if flags & TFHD_DEFAULT_SAMPLE_FLAGS_PRESENT != 0 {
        atom.read_u32()?
    } else {
        trex_entry.map_or(0, |t| t.default_sample_flags)
    };

    Ok(TfhdBox {
        track_id,
        base_data_offset,
        default_base_is_moof: flags & TFHD_DEFAULT_BASE_IS_MOOF != 0,
        defaults: TrafDefaults {
            duration,
            size,
            flags: sample_flags,
        },
    })
}

/// Parses a `tfdt` box, returning `base_media_decode_time`.
fn parse_tfdt(data: &[u8]) -> OxiResult<u64> {
    let mut atom = Mp4Atom::new(data);
    let version = atom.read_u8()?;
    atom.skip(3)?; // flags
    if version == 1 {
        atom.read_u64()
    } else {
        Ok(u64::from(atom.read_u32()?))
    }
}

/// Parses one `traf` box, appending its samples to `out`.
fn parse_traf(
    data: &[u8],
    moof_offset: u64,
    trex: &[TrexBox],
    decode_times: &mut HashMap<u32, u64>,
    previous_traf_end: &mut Option<u64>,
    out: &mut Vec<FragmentSample>,
) -> OxiResult<()> {
    // Collect children first: `trun` needs `tfhd` (and, when present, `tfdt`)
    // to already be resolved, and while the spec mandates `tfhd` first we do
    // not want to depend on writer ordering.
    let mut tfhd: Option<TfhdBox> = None;
    let mut tfdt: Option<u64> = None;
    let mut truns: Vec<&[u8]> = Vec::new();

    for (box_type, content) in ChildBoxes::new(data) {
        match box_type {
            BoxType::TFHD => tfhd = Some(parse_tfhd(content, trex)?),
            BoxType::TFDT => tfdt = Some(parse_tfdt(content)?),
            BoxType::TRUN => truns.push(content),
            _ => {}
        }
    }

    let Some(tfhd) = tfhd else {
        return Err(OxiError::Parse {
            offset: moof_offset,
            message: "traf box has no tfhd".into(),
        });
    };

    let base_data_offset = match tfhd.base_data_offset {
        Some(explicit) => explicit,
        None if tfhd.default_base_is_moof => moof_offset,
        None => previous_traf_end.unwrap_or(moof_offset),
    };

    let mut dts = tfdt.unwrap_or_else(|| {
        decode_times
            .get(&tfhd.track_id)
            .copied()
            .unwrap_or_default()
    });
    // Where the next `trun` without an explicit `data_offset` starts.
    let mut running_data_pos = base_data_offset;

    for trun in truns {
        running_data_pos = parse_trun(
            trun,
            &tfhd,
            base_data_offset,
            running_data_pos,
            &mut dts,
            out,
        )?;
    }

    decode_times.insert(tfhd.track_id, dts);
    *previous_traf_end = Some(running_data_pos);
    Ok(())
}

/// Parses one `trun` box, appending its samples to `out`.
///
/// Returns the absolute byte offset just past the last sample of this run,
/// which becomes the implicit start of the next run.
fn parse_trun(
    data: &[u8],
    tfhd: &TfhdBox,
    base_data_offset: u64,
    running_data_pos: u64,
    dts: &mut u64,
    out: &mut Vec<FragmentSample>,
) -> OxiResult<u64> {
    let mut atom = Mp4Atom::new(data);
    let version = atom.read_u8()?;
    let flags = u32::from(atom.read_u8()?) << 16
        | u32::from(atom.read_u8()?) << 8
        | u32::from(atom.read_u8()?);

    let sample_count = atom.read_u32()? as usize;

    let mut data_pos = if flags & TRUN_DATA_OFFSET_PRESENT != 0 {
        let delta = i64::from(atom.read_i32()?);
        apply_signed_offset(base_data_offset, delta)?
    } else {
        running_data_pos
    };

    let first_sample_flags = if flags & TRUN_FIRST_SAMPLE_FLAGS_PRESENT != 0 {
        Some(atom.read_u32()?)
    } else {
        None
    };

    // Reject a `sample_count` that cannot possibly fit in the remaining bytes
    // before allocating anything for it.
    let mut record_size = 0usize;
    for present in [
        TRUN_SAMPLE_DURATION_PRESENT,
        TRUN_SAMPLE_SIZE_PRESENT,
        TRUN_SAMPLE_FLAGS_PRESENT,
        TRUN_SAMPLE_CTS_PRESENT,
    ] {
        if flags & present != 0 {
            record_size += 4;
        }
    }
    let available = atom.remaining().len();
    if sample_count.saturating_mul(record_size) > available || sample_count > MAX_TRUN_SAMPLE_COUNT
    {
        return Err(OxiError::Parse {
            offset: base_data_offset,
            message: format!(
                "trun: declared sample_count {sample_count} needs {} bytes but only {available} bytes remain",
                sample_count.saturating_mul(record_size)
            ),
        });
    }

    out.reserve(sample_count);
    for index in 0..sample_count {
        let duration = if flags & TRUN_SAMPLE_DURATION_PRESENT != 0 {
            atom.read_u32()?
        } else {
            tfhd.defaults.duration
        };
        let size = if flags & TRUN_SAMPLE_SIZE_PRESENT != 0 {
            atom.read_u32()?
        } else {
            tfhd.defaults.size
        };
        let sample_flags = if flags & TRUN_SAMPLE_FLAGS_PRESENT != 0 {
            atom.read_u32()?
        } else if index == 0 {
            first_sample_flags.unwrap_or(tfhd.defaults.flags)
        } else {
            tfhd.defaults.flags
        };
        let cts_offset = if flags & TRUN_SAMPLE_CTS_PRESENT != 0 {
            if version == 0 {
                // Version 0 stores an unsigned offset; values above i32::MAX
                // are not representable and are clamped rather than wrapped.
                i32::try_from(atom.read_u32()?).unwrap_or(i32::MAX)
            } else {
                atom.read_i32()?
            }
        } else {
            0
        };

        out.push(FragmentSample {
            track_id: tfhd.track_id,
            offset: data_pos,
            size,
            duration,
            cts_offset,
            dts: *dts,
            is_sync: sample_flags & SAMPLE_IS_NON_SYNC == 0,
        });

        data_pos = data_pos
            .checked_add(u64::from(size))
            .ok_or_else(|| OxiError::Parse {
                offset: base_data_offset,
                message: "trun: sample offset overflows 64 bits".into(),
            })?;
        *dts = dts
            .checked_add(u64::from(duration))
            .ok_or_else(|| OxiError::Parse {
                offset: base_data_offset,
                message: "trun: decode time overflows 64 bits".into(),
            })?;
    }

    Ok(data_pos)
}

/// Adds a signed delta to an unsigned base offset, erroring on overflow or a
/// negative result.
fn apply_signed_offset(base: u64, delta: i64) -> OxiResult<u64> {
    let signed_base = i64::try_from(base).map_err(|_| OxiError::Parse {
        offset: 0,
        message: "fragment base offset exceeds i64 range".into(),
    })?;
    let result = signed_base
        .checked_add(delta)
        .ok_or_else(|| OxiError::Parse {
            offset: 0,
            message: "fragment data offset overflows".into(),
        })?;
    u64::try_from(result).map_err(|_| OxiError::Parse {
        offset: 0,
        message: "fragment data offset is negative".into(),
    })
}

// ─── Child-box iteration ─────────────────────────────────────────────────────

/// Iterates the direct children of a container box's content.
///
/// Stops silently on the first malformed/truncated child, matching the
/// resilient behaviour of the rest of the MP4 parser.
struct ChildBoxes<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ChildBoxes<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }
}

impl<'a> Iterator for ChildBoxes<'a> {
    type Item = (BoxType, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + 8 > self.data.len() {
            return None;
        }
        let remaining = &self.data[self.offset..];
        let header = BoxHeader::parse(remaining).ok()?;
        let header_size = header.header_size as usize;
        let box_total = if header.size == 0 {
            remaining.len()
        } else {
            usize::try_from(header.size).ok()?
        };
        let end_off = self.offset.checked_add(box_total)?;
        if box_total < header_size || end_off > self.data.len() {
            return None;
        }
        let content = &self.data[self.offset + header_size..end_off];
        self.offset = end_off;
        Some((header.box_type, content))
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn full_box(tag: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + payload.len());
        body.push(version);
        body.push(((flags >> 16) & 0xFF) as u8);
        body.push(((flags >> 8) & 0xFF) as u8);
        body.push((flags & 0xFF) as u8);
        body.extend_from_slice(payload);
        plain_box(tag, &body)
    }

    fn plain_box(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let total = (payload.len() + 8) as u32;
        let mut out = Vec::with_capacity(total as usize);
        out.extend_from_slice(&total.to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(payload);
        out
    }

    fn trex_payload(track_id: u32, duration: u32, size: u32, flags: u32) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&track_id.to_be_bytes());
        c.extend_from_slice(&1u32.to_be_bytes());
        c.extend_from_slice(&duration.to_be_bytes());
        c.extend_from_slice(&size.to_be_bytes());
        c.extend_from_slice(&flags.to_be_bytes());
        c
    }

    #[test]
    fn parse_mvex_reads_every_trex() {
        let mut mvex = Vec::new();
        mvex.extend(full_box(b"trex", 0, 0, &trex_payload(1, 3000, 128, 0)));
        mvex.extend(full_box(b"trex", 0, 0, &trex_payload(2, 960, 64, 0)));
        let entries = parse_mvex(&mvex).expect("mvex parses");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].track_id, 1);
        assert_eq!(entries[0].default_sample_duration, 3000);
        assert_eq!(entries[1].default_sample_size, 64);
    }

    /// A `trun` carrying every optional per-sample field, addressed via
    /// `default-base-is-moof`.
    #[test]
    fn parse_moof_full_trun() {
        let moof_offset = 1000u64;
        let mut tfhd_c = Vec::new();
        tfhd_c.extend_from_slice(&7u32.to_be_bytes()); // track_id
        let tfhd = full_box(b"tfhd", 0, TFHD_DEFAULT_BASE_IS_MOOF, &tfhd_c);
        let tfdt = full_box(b"tfdt", 1, 0, &5000u64.to_be_bytes());

        let mut trun_c = Vec::new();
        trun_c.extend_from_slice(&2u32.to_be_bytes()); // sample_count
        trun_c.extend_from_slice(&200i32.to_be_bytes()); // data_offset
                                                         // sample 0 (sync)
        trun_c.extend_from_slice(&100u32.to_be_bytes()); // duration
        trun_c.extend_from_slice(&10u32.to_be_bytes()); // size
        trun_c.extend_from_slice(&0x0200_0000u32.to_be_bytes()); // flags: sync
        trun_c.extend_from_slice(&0i32.to_be_bytes()); // cts
                                                       // sample 1 (non-sync, negative cts)
        trun_c.extend_from_slice(&120u32.to_be_bytes());
        trun_c.extend_from_slice(&20u32.to_be_bytes());
        trun_c.extend_from_slice(&0x0101_0000u32.to_be_bytes());
        trun_c.extend_from_slice(&(-30i32).to_be_bytes());
        let trun_flags = TRUN_DATA_OFFSET_PRESENT
            | TRUN_SAMPLE_DURATION_PRESENT
            | TRUN_SAMPLE_SIZE_PRESENT
            | TRUN_SAMPLE_FLAGS_PRESENT
            | TRUN_SAMPLE_CTS_PRESENT;
        let trun = full_box(b"trun", 1, trun_flags, &trun_c);

        let mut traf_c = Vec::new();
        traf_c.extend(tfhd);
        traf_c.extend(tfdt);
        traf_c.extend(trun);
        let traf = plain_box(b"traf", &traf_c);

        let mut moof_c = Vec::new();
        moof_c.extend(full_box(b"mfhd", 0, 0, &3u32.to_be_bytes()));
        moof_c.extend(traf);

        let mut decode_times = HashMap::new();
        let info = parse_moof(&moof_c, moof_offset, &[], &mut decode_times).expect("moof parses");

        assert_eq!(info.sequence_number, 3);
        assert_eq!(info.samples.len(), 2);

        assert_eq!(info.samples[0].track_id, 7);
        assert_eq!(info.samples[0].offset, moof_offset + 200);
        assert_eq!(info.samples[0].size, 10);
        assert_eq!(info.samples[0].duration, 100);
        assert_eq!(info.samples[0].dts, 5000);
        assert_eq!(info.samples[0].cts_offset, 0);
        assert!(info.samples[0].is_sync);

        assert_eq!(info.samples[1].offset, moof_offset + 210);
        assert_eq!(info.samples[1].dts, 5100);
        assert_eq!(info.samples[1].cts_offset, -30);
        assert!(!info.samples[1].is_sync);

        assert_eq!(decode_times.get(&7).copied(), Some(5220));
    }

    /// A `trun` with no per-sample fields at all falls back to `trex` defaults,
    /// and a missing `tfdt` continues the running decode time.
    #[test]
    fn parse_moof_defaults_from_trex_and_running_dts() {
        let trex = vec![TrexBox {
            track_id: 1,
            default_sample_description_index: 1,
            default_sample_duration: 512,
            default_sample_size: 8,
            default_sample_flags: 0,
        }];

        let mut tfhd_c = Vec::new();
        tfhd_c.extend_from_slice(&1u32.to_be_bytes());
        let tfhd = full_box(b"tfhd", 0, TFHD_DEFAULT_BASE_IS_MOOF, &tfhd_c);

        let mut trun_c = Vec::new();
        trun_c.extend_from_slice(&3u32.to_be_bytes()); // sample_count
        trun_c.extend_from_slice(&64i32.to_be_bytes()); // data_offset
        let trun = full_box(b"trun", 0, TRUN_DATA_OFFSET_PRESENT, &trun_c);

        let mut traf_c = Vec::new();
        traf_c.extend(tfhd);
        traf_c.extend(trun);
        let moof_c = plain_box(b"traf", &traf_c);

        let mut decode_times = HashMap::new();
        decode_times.insert(1u32, 2048u64);
        let info = parse_moof(&moof_c, 0, &trex, &mut decode_times).expect("moof parses");

        assert_eq!(info.samples.len(), 3);
        for (i, sample) in info.samples.iter().enumerate() {
            assert_eq!(sample.size, 8);
            assert_eq!(sample.duration, 512);
            assert!(sample.is_sync, "default flags 0 means sync");
            assert_eq!(sample.offset, 64 + 8 * i as u64);
            assert_eq!(sample.dts, 2048 + 512 * i as u64);
        }
        assert_eq!(decode_times.get(&1).copied(), Some(2048 + 3 * 512));
    }

    /// `first-sample-flags-present` must apply to sample 0 only.
    #[test]
    fn parse_moof_first_sample_flags() {
        let mut tfhd_c = Vec::new();
        tfhd_c.extend_from_slice(&1u32.to_be_bytes());
        // default sample flags mark every sample non-sync
        tfhd_c.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        let tfhd = full_box(
            b"tfhd",
            0,
            TFHD_DEFAULT_BASE_IS_MOOF | TFHD_DEFAULT_SAMPLE_FLAGS_PRESENT,
            &tfhd_c,
        );

        let mut trun_c = Vec::new();
        trun_c.extend_from_slice(&2u32.to_be_bytes());
        trun_c.extend_from_slice(&8i32.to_be_bytes());
        trun_c.extend_from_slice(&0x0200_0000u32.to_be_bytes()); // first_sample_flags: sync
        trun_c.extend_from_slice(&5u32.to_be_bytes()); // size sample 0
        trun_c.extend_from_slice(&6u32.to_be_bytes()); // size sample 1
        let trun = full_box(
            b"trun",
            0,
            TRUN_DATA_OFFSET_PRESENT | TRUN_FIRST_SAMPLE_FLAGS_PRESENT | TRUN_SAMPLE_SIZE_PRESENT,
            &trun_c,
        );

        let mut traf_c = Vec::new();
        traf_c.extend(tfhd);
        traf_c.extend(trun);
        let moof_c = plain_box(b"traf", &traf_c);

        let mut decode_times = HashMap::new();
        let info = parse_moof(&moof_c, 0, &[], &mut decode_times).expect("moof parses");
        assert_eq!(info.samples.len(), 2);
        assert!(info.samples[0].is_sync, "first sample flags say sync");
        assert!(!info.samples[1].is_sync, "later samples use tfhd default");
        assert_eq!(info.samples[0].size, 5);
        assert_eq!(info.samples[1].size, 6);
    }

    /// Two `traf` boxes without explicit base offsets: the second continues
    /// where the first one's data ended.
    #[test]
    fn parse_moof_second_traf_continues_previous_data() {
        let build_traf = |track_id: u32, size: u32, with_data_offset: Option<i32>| {
            let mut tfhd_c = Vec::new();
            tfhd_c.extend_from_slice(&track_id.to_be_bytes());
            let tfhd = full_box(b"tfhd", 0, 0, &tfhd_c);

            let mut trun_c = Vec::new();
            trun_c.extend_from_slice(&1u32.to_be_bytes());
            let mut flags = TRUN_SAMPLE_SIZE_PRESENT;
            if let Some(offset) = with_data_offset {
                flags |= TRUN_DATA_OFFSET_PRESENT;
                trun_c.extend_from_slice(&offset.to_be_bytes());
            }
            trun_c.extend_from_slice(&size.to_be_bytes());
            let trun = full_box(b"trun", 0, flags, &trun_c);

            let mut traf_c = Vec::new();
            traf_c.extend(tfhd);
            traf_c.extend(trun);
            plain_box(b"traf", &traf_c)
        };

        let mut moof_c = Vec::new();
        moof_c.extend(build_traf(1, 40, Some(100)));
        moof_c.extend(build_traf(2, 10, None));

        let mut decode_times = HashMap::new();
        let info = parse_moof(&moof_c, 500, &[], &mut decode_times).expect("moof parses");
        assert_eq!(info.samples.len(), 2);
        assert_eq!(info.samples[0].offset, 600, "500 + data_offset 100");
        assert_eq!(
            info.samples[1].offset, 640,
            "continues after the first traf's 40 bytes"
        );
    }

    #[test]
    fn parse_trun_rejects_impossible_sample_count() {
        let mut tfhd_c = Vec::new();
        tfhd_c.extend_from_slice(&1u32.to_be_bytes());
        let tfhd = full_box(b"tfhd", 0, TFHD_DEFAULT_BASE_IS_MOOF, &tfhd_c);

        let mut trun_c = Vec::new();
        trun_c.extend_from_slice(&u32::MAX.to_be_bytes()); // sample_count
        trun_c.extend_from_slice(&0i32.to_be_bytes());
        trun_c.extend_from_slice(&[0u8; 8]); // nowhere near enough payload
        let trun = full_box(
            b"trun",
            0,
            TRUN_DATA_OFFSET_PRESENT | TRUN_SAMPLE_SIZE_PRESENT | TRUN_SAMPLE_DURATION_PRESENT,
            &trun_c,
        );

        let mut traf_c = Vec::new();
        traf_c.extend(tfhd);
        traf_c.extend(trun);
        let moof_c = plain_box(b"traf", &traf_c);

        let mut decode_times = HashMap::new();
        assert!(parse_moof(&moof_c, 0, &[], &mut decode_times).is_err());
    }

    #[test]
    fn parse_trun_rejects_unbounded_zero_record_count() {
        // Every optional field absent → record_size == 0, so the
        // bytes-remaining check alone cannot bound the count.
        let mut tfhd_c = Vec::new();
        tfhd_c.extend_from_slice(&1u32.to_be_bytes());
        let tfhd = full_box(b"tfhd", 0, TFHD_DEFAULT_BASE_IS_MOOF, &tfhd_c);

        let trun = full_box(b"trun", 0, 0, &u32::MAX.to_be_bytes());
        let mut traf_c = Vec::new();
        traf_c.extend(tfhd);
        traf_c.extend(trun);
        let moof_c = plain_box(b"traf", &traf_c);

        let mut decode_times = HashMap::new();
        assert!(parse_moof(&moof_c, 0, &[], &mut decode_times).is_err());
    }

    #[test]
    fn parse_traf_without_tfhd_is_an_error() {
        let trun = full_box(b"trun", 0, 0, &0u32.to_be_bytes());
        let moof_c = plain_box(b"traf", &trun);
        let mut decode_times = HashMap::new();
        assert!(parse_moof(&moof_c, 0, &[], &mut decode_times).is_err());
    }

    #[test]
    fn explicit_base_data_offset_wins() {
        let mut tfhd_c = Vec::new();
        tfhd_c.extend_from_slice(&1u32.to_be_bytes());
        tfhd_c.extend_from_slice(&9000u64.to_be_bytes()); // base_data_offset
        let tfhd = full_box(
            b"tfhd",
            0,
            TFHD_BASE_DATA_OFFSET_PRESENT | TFHD_DEFAULT_BASE_IS_MOOF,
            &tfhd_c,
        );

        let mut trun_c = Vec::new();
        trun_c.extend_from_slice(&1u32.to_be_bytes());
        trun_c.extend_from_slice(&16i32.to_be_bytes());
        trun_c.extend_from_slice(&4u32.to_be_bytes());
        let trun = full_box(
            b"trun",
            0,
            TRUN_DATA_OFFSET_PRESENT | TRUN_SAMPLE_SIZE_PRESENT,
            &trun_c,
        );

        let mut traf_c = Vec::new();
        traf_c.extend(tfhd);
        traf_c.extend(trun);
        let moof_c = plain_box(b"traf", &traf_c);

        let mut decode_times = HashMap::new();
        let info = parse_moof(&moof_c, 123, &[], &mut decode_times).expect("moof parses");
        assert_eq!(info.samples[0].offset, 9016);
    }

    #[test]
    fn child_boxes_stops_on_truncation() {
        let mut data = plain_box(b"mfhd", &[0u8; 8]);
        // A header claiming a size larger than the buffer.
        data.extend_from_slice(&999u32.to_be_bytes());
        data.extend_from_slice(b"traf");
        let children: Vec<_> = ChildBoxes::new(&data).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].0, BoxType::MFHD);
    }
}
