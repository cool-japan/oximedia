//! Tests for the AV1 intra-frame reconstruction driver and its
//! filter-stage row-band split (see [`super`]).
//!
//! Kept in a sibling file so `recon.rs` stays under the 2000-line
//! limit; `include_bytes!` paths resolve against this directory.

use super::super::parse_tile_group;
use super::{auto_band_count, decode_intra_frame_banded, plan_bands, split_out_bands, PlaneBuf};
use crate::av1::kf::hdr::{FrameHdr, SeqHdr};
use crate::av1::obu::{ObuIterator, ObuType};

#[test]
fn plan_bands_partitions_exactly() {
    for total in [4usize, 8, 12, 48, 132, 4096] {
        for align in [2usize, 4] {
            if total % align != 0 {
                continue;
            }
            for count in 1..=12usize {
                let bounds = plan_bands(total, align, count);
                assert!(!bounds.is_empty());
                assert_eq!(bounds[0].0, 0, "bands start at row 0");
                assert_eq!(
                    bounds[bounds.len() - 1].1,
                    total,
                    "bands cover the whole height"
                );
                assert!(bounds.len() <= count.max(1));
                for (i, &(a, b)) in bounds.iter().enumerate() {
                    assert!(a < b, "band {i} of {bounds:?} is empty");
                    assert_eq!(a % align, 0, "band {i} start is unaligned");
                    if i + 1 < bounds.len() {
                        assert_eq!(b % align, 0, "band {i} end is unaligned");
                        assert_eq!(b, bounds[i + 1].0, "band {i} is not contiguous");
                    }
                }
            }
        }
    }
    // A zero-height plane has nothing to band: no bands, never an empty one.
    for align in [2usize, 4] {
        for count in [0usize, 1, 8] {
            assert!(plan_bands(0, align, count).is_empty());
        }
    }
}

#[test]
fn split_out_bands_covers_every_sample_once() {
    // 4:2:0 planes for a 16x12 MI frame: 64x48 luma, 32x24 chroma.
    let (lw, lh) = (64usize, 48usize);
    let mut out: [Vec<u8>; 3] = [
        vec![0u8; lw * lh],
        vec![0u8; (lw / 2) * (lh / 2)],
        vec![0u8; (lw / 2) * (lh / 2)],
    ];
    let strides = [lw, lw / 2, lw / 2];
    let bounds = plan_bands(lh, 4, 5);
    {
        let bands = split_out_bands(&mut out, strides, 1, &bounds);
        assert_eq!(bands.len(), bounds.len());
        for (band_no, (mut band, &(y0, y1))) in bands.into_iter().zip(bounds.iter()).enumerate() {
            for plane in 0..3 {
                let shift = usize::from(plane > 0);
                for y in (y0 >> shift)..(y1 >> shift) {
                    for x in 0..strides[plane] {
                        band.put(plane, y, x, band_no as u8 + 1);
                    }
                }
            }
        }
    }
    // Every sample written exactly once, by the band owning its row.
    for (plane, data) in out.iter().enumerate() {
        let shift = usize::from(plane > 0);
        for (i, &v) in data.iter().enumerate() {
            let y = i / strides[plane];
            let owner = bounds
                .iter()
                .position(|&(a, b)| y >= (a >> shift) && y < (b >> shift))
                .expect("every row belongs to a band");
            assert_eq!(
                v,
                owner as u8 + 1,
                "plane {plane} row {y} written by a band"
            );
        }
    }
}

#[test]
fn auto_band_count_is_serial_for_small_frames() {
    assert_eq!(auto_band_count(64 * 64, 1 << 16), 1);
    assert_eq!(auto_band_count(0, 1 << 16), 1);
    assert!(auto_band_count(1920 * 1088, 1 << 16) >= 1);
    assert!(auto_band_count(1920 * 1088, 1 << 16) <= rayon::current_num_threads());
}

/// Parses one temporal unit down to the pieces `decode_intra_frame`
/// needs, mirroring [`crate::av1::kf::decode_temporal_unit`].
fn split_tu(tu: &[u8]) -> (SeqHdr, FrameHdr, Vec<&[u8]>) {
    let mut seq: Option<SeqHdr> = None;
    let mut hdr: Option<FrameHdr> = None;
    let mut tiles: Vec<&[u8]> = Vec::new();
    for obu in ObuIterator::new(tu) {
        let (header, payload) = obu.expect("obu parse");
        match header.obu_type {
            ObuType::SequenceHeader => {
                seq = Some(SeqHdr::parse(payload).expect("seq parse"));
            }
            ObuType::Frame => {
                let s = seq.as_ref().expect("sequence header before frame");
                let f = FrameHdr::parse(payload, s).expect("frame parse");
                let header_bytes = f.header_bits.div_ceil(8);
                parse_tile_group(&payload[header_bytes..], &f, &mut tiles).expect("tile group");
                hdr = Some(f);
            }
            ObuType::FrameHeader => {
                let s = seq.as_ref().expect("sequence header before frame");
                hdr = Some(FrameHdr::parse(payload, s).expect("frame parse"));
            }
            ObuType::TileGroup => {
                let f = hdr.as_ref().expect("tile group before frame header");
                parse_tile_group(payload, f, &mut tiles).expect("tile group");
            }
            _ => {}
        }
    }
    (
        seq.expect("sequence header"),
        hdr.expect("frame header"),
        tiles,
    )
}

/// End-to-end proof that the CDEF and loop-restoration row-band split
/// never changes a decoded sample: every committed fixture is decoded
/// with 1..=8 bands and compared against the dav1d/aomdec reference YUV
/// the serial decoder is already validated against. The auto band count
/// keeps these small frames serial, so this is the only place the
/// parallel stage code runs on real bitstreams.
#[test]
fn filter_stage_bands_bit_exact() {
    let vectors: [(&[u8], &[u8], usize, usize, &str); 6] = [
        (
            include_bytes!("testdata/s3_aom128.obu"),
            include_bytes!("testdata/s3_aom128.yuv"),
            128,
            128,
            "s3_aom128 (cdef)",
        ),
        (
            include_bytes!("testdata/s4_aom128.obu"),
            include_bytes!("testdata/s4_aom128.yuv"),
            128,
            128,
            "s4_aom128 (cdef, lr none)",
        ),
        (
            include_bytes!("testdata/s5_lr320_q30.obu"),
            include_bytes!("testdata/s5_lr320_q30.yuv"),
            320,
            192,
            "s5_lr320_q30 (sgrproj)",
        ),
        (
            include_bytes!("testdata/s5_blur_q20.obu"),
            include_bytes!("testdata/s5_blur_q20.yuv"),
            320,
            192,
            "s5_blur_q20 (switchable wiener)",
        ),
        (
            include_bytes!("testdata/s2_aom76x42.obu"),
            include_bytes!("testdata/s2_aom76x42.yuv"),
            76,
            42,
            "s2_aom76x42 (odd size)",
        ),
        (
            include_bytes!("testdata/s1_svt256tc.obu"),
            include_bytes!("testdata/s1_svt256tc.yuv"),
            256,
            128,
            "s1_svt256tc (two tiles)",
        ),
    ];
    for (tu, r#ref, w, h, label) in vectors {
        let (seq, hdr, tiles) = split_tu(tu);
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        assert_eq!(r#ref.len(), w * h + 2 * cw * ch, "{label}: reference size");
        for bands in 1..=8usize {
            let frame = decode_intra_frame_banded(&seq, &hdr, &tiles, Some(bands))
                .unwrap_or_else(|e| panic!("{label}: decode with {bands} bands failed: {e}"));
            let mut off = 0usize;
            for (plane, (pw, ph)) in [(w, h), (cw, ch), (cw, ch)].into_iter().enumerate() {
                let p = &frame.planes[plane];
                for y in 0..ph {
                    for x in 0..pw {
                        assert_eq!(
                            p.data[y * p.stride + x],
                            r#ref[off + y * pw + x],
                            "{label}: plane {plane} ({x},{y}) differs with {bands} bands"
                        );
                    }
                }
                off += pw * ph;
            }
        }
    }
}

// ------------------------------------------------------------- timing

/// Deterministic filler (splitmix64) for the synthetic perf frames.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn perf_planes(mi_rows: usize, mi_cols: usize) -> [PlaneBuf; 3] {
    let mut state = 0x0BAD_C0DE;
    let mut mk = |w: usize, h: usize| PlaneBuf {
        data: (0..w * h).map(|_| next(&mut state) as u8).collect(),
        stride: w,
        width: w,
        height: h,
    };
    let (lw, lh) = (mi_cols * 4, mi_rows * 4);
    [mk(lw, lh), mk(lw / 2, lh / 2), mk(lw / 2, lh / 2)]
}

/// Best of `runs` wall-clock samples (least noisy estimator here).
fn best_of(runs: usize, mut f: impl FnMut()) -> std::time::Duration {
    let mut best = std::time::Duration::MAX;
    for _ in 0..runs {
        let t0 = std::time::Instant::now();
        f();
        best = best.min(t0.elapsed());
    }
    best
}

/// Wall-clock comparison of the serial and row-band-parallel filter
/// stages. Ignored by default; run it with
///
/// ```text
/// cargo nextest run -p oximedia-codec --release --run-ignored all \
///     --no-capture perf_filter_stages
/// ```
///
/// Set `OXIMEDIA_AV1_PERF_OBU` to a single-temporal-unit AV1 keyframe
/// (`ffmpeg -i in -c:v libaom-av1 -frames:v 1 -f obu out.obu`) to also
/// get the end-to-end decode figure; the committed fixtures top out at
/// 320x192, far too small for the split to pay off.
#[test]
#[ignore = "wall-clock benchmark; needs --release and is machine dependent"]
fn perf_filter_stages() {
    let threads = rayon::current_num_threads();
    // 1920x1088 = 480x272 MI.
    let (mi_rows, mi_cols) = (272usize, 480usize);
    let n = mi_rows * mi_cols;
    let mut state = 0x5EED_5EED;
    let skips: Vec<u8> = (0..n)
        .map(|_| u8::from(next(&mut state) & 3 == 0))
        .collect();
    let cdef_idx: Vec<i16> = (0..n).map(|_| (next(&mut state) % 8) as i16).collect();
    let mut hdr = FrameHdr::default();
    hdr.frame_width = (mi_cols * 4) as u32;
    hdr.frame_height = (mi_rows * 4) as u32;
    hdr.upscaled_width = hdr.frame_width;
    hdr.cdef.damping = 4;
    hdr.cdef.bits = 3;
    for i in 0..8 {
        hdr.cdef.y_pri_strength[i] = 1 + (i as u32);
        hdr.cdef.y_sec_strength[i] = [0u32, 1, 2, 4][i % 4];
        hdr.cdef.uv_pri_strength[i] = 1 + (i as u32) % 5;
        hdr.cdef.uv_sec_strength[i] = [0u32, 1, 2, 4][(i + 1) % 4];
    }
    let cdef_in = crate::av1::kf::cdef::CdefInput {
        hdr: &hdr,
        sub_x: true,
        sub_y: true,
        num_planes: 3,
        mi_rows,
        mi_cols,
        skips: &skips,
        cdef_idx: &cdef_idx,
    };
    let base = perf_planes(mi_rows, mi_cols);
    let clone_planes = |src: &[PlaneBuf; 3]| {
        [0usize, 1, 2].map(|i| PlaneBuf {
            data: src[i].data.clone(),
            stride: src[i].stride,
            width: src[i].width,
            height: src[i].height,
        })
    };
    let cdef_serial = best_of(3, || {
        let mut p = clone_planes(&base);
        crate::av1::kf::cdef::cdef_frame(&mut p, &cdef_in, Some(1));
    });
    let cdef_par = best_of(3, || {
        let mut p = clone_planes(&base);
        crate::av1::kf::cdef::cdef_frame(&mut p, &cdef_in, None);
    });

    hdr.lr.uses_lr = true;
    hdr.lr.frame_restoration_type = [
        crate::av1::kf::consts::RESTORE_SGRPROJ,
        crate::av1::kf::consts::RESTORE_WIENER,
        crate::av1::kf::consts::RESTORE_WIENER,
    ];
    hdr.lr.loop_restoration_size = [64, 32, 32];
    let mut grids = crate::av1::kf::lr::LrUnitGrids::new(&hdr, true, true, 3);
    for plane in 0..3 {
        let units = grids.unit_rows[plane] * grids.unit_cols[plane];
        for idx in 0..units {
            grids.types[plane][idx] = hdr.lr.frame_restoration_type[plane] as u8;
            grids.sgr_set[plane][idx] = (idx % 16) as u8;
            grids.sgr_xqd[plane][idx] = [32, 32];
            grids.wiener[plane][idx] = [[3, -7, 15], [3, -7, 15]];
        }
    }
    let pre = perf_planes(mi_rows, mi_cols);
    let apply = crate::av1::kf::lr::LrApply {
        hdr: &hdr,
        sub_x: true,
        sub_y: true,
        num_planes: 3,
        pre_cdef: &pre,
        grids: &grids,
    };
    let lr_serial = best_of(3, || {
        let mut p = clone_planes(&base);
        crate::av1::kf::lr::loop_restore_frame(&mut p, &apply, Some(1));
    });
    let lr_par = best_of(3, || {
        let mut p = clone_planes(&base);
        crate::av1::kf::lr::loop_restore_frame(&mut p, &apply, None);
    });

    let ratio = |s: std::time::Duration, p: std::time::Duration| {
        s.as_secs_f64() / p.as_secs_f64().max(f64::MIN_POSITIVE)
    };
    println!("rayon threads: {threads}");
    println!(
        "CDEF  1920x1088: serial {:>9.3?}  banded {:>9.3?}  speedup {:.2}x",
        cdef_serial,
        cdef_par,
        ratio(cdef_serial, cdef_par)
    );
    println!(
        "LR    1920x1088: serial {:>9.3?}  banded {:>9.3?}  speedup {:.2}x",
        lr_serial,
        lr_par,
        ratio(lr_serial, lr_par)
    );

    let Ok(path) = std::env::var("OXIMEDIA_AV1_PERF_OBU") else {
        println!("OXIMEDIA_AV1_PERF_OBU unset: skipping the end-to-end frame decode");
        return;
    };
    let tu = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let (seq, fh, tiles) = split_tu(&tu);
    println!(
        "frame: {}x{}, {} tile(s), cdef={} lr={}",
        fh.frame_width,
        fh.frame_height,
        tiles.len(),
        seq.enable_cdef,
        fh.lr.uses_lr
    );
    // A real stream may use a surface this decoder honestly rejects
    // (palette, intrabc, superres, ...); report that instead of failing
    // a benchmark on it.
    let reference = match decode_intra_frame_banded(&seq, &fh, &tiles, Some(1)) {
        Ok(f) => f,
        Err(e) => {
            println!("end-to-end skipped: {e}");
            return;
        }
    };
    let banded_frame = decode_intra_frame_banded(&seq, &fh, &tiles, None).expect("banded decode");
    for plane in 0..3 {
        assert_eq!(
            reference.planes[plane].data, banded_frame.planes[plane].data,
            "perf frame plane {plane} differs between the serial and banded decode"
        );
    }
    let serial = best_of(3, || {
        decode_intra_frame_banded(&seq, &fh, &tiles, Some(1)).expect("serial decode");
    });
    let banded = best_of(3, || {
        decode_intra_frame_banded(&seq, &fh, &tiles, None).expect("banded decode");
    });
    println!(
        "whole frame:     serial {:>9.3?}  banded {:>9.3?}  speedup {:.2}x",
        serial,
        banded,
        ratio(serial, banded)
    );
}
