//! Scalar-vs-SIMD equivalence tests for the AV1 loop filter kernels.
//!
//! The oracle is [`super::filter_edge4_reference`], a literal transcription of
//! spec 7.14.6 applied to one line at a time.  Every optimised path must be
//! byte-identical to it.
//!
//! Uniform-random samples are almost useless here: `filter_mask` rejects most
//! random 14-tuples outright and `flat_mask` (all neighbours within ±1 of
//! `p0`/`q0`) essentially never fires, so a naive sweep would never reach the
//! wide filters at all.  The generators below are therefore *conditioned* on
//! the parameters under test, and every sweep asserts on a branch-hit
//! histogram so "N cases passed" cannot be vacuous.

use super::{Av1Edge4, Av1FilterSize, Av1LfParams};

// ── Deterministic PRNG ───────────────────────────────────────────────────────

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u32(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 32) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next_u32() % n
        }
    }
    fn byte(&mut self) -> u8 {
        self.next_u32() as u8
    }
    /// Uniform in `[-d, d]`.
    fn jitter(&mut self, d: i32) -> i32 {
        if d <= 0 {
            0
        } else {
            self.below((2 * d + 1) as u32) as i32 - d
        }
    }
}

fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

// ── Reachable parameter space ────────────────────────────────────────────────

/// Independent transcription of the adaptive filter strength process
/// (spec 7.14.4) so the sweep covers exactly the `(limit, blimit, thresh)`
/// triples a real bitstream can produce.
fn strength(lvl: i32, sharpness: i32) -> (i32, i32, i32) {
    let shift = if sharpness > 4 {
        2
    } else if sharpness > 0 {
        1
    } else {
        0
    };
    let limit = if sharpness > 0 {
        (lvl >> shift).clamp(1, 9 - sharpness)
    } else {
        core::cmp::max(1, lvl >> shift)
    };
    let blimit = 2 * (lvl + 2) + limit;
    let thresh = lvl >> 4;
    (limit, blimit, thresh)
}

// ── Branch classification ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    Skipped,
    NarrowHev,
    NarrowNoHev,
    Wide8,
    Wide14,
}

#[derive(Default, Debug, Clone, Copy)]
struct Hits {
    skipped: u64,
    narrow_hev: u64,
    narrow_no_hev: u64,
    wide8: u64,
    wide14: u64,
}

impl Hits {
    fn record(&mut self, b: Branch) {
        match b {
            Branch::Skipped => self.skipped += 1,
            Branch::NarrowHev => self.narrow_hev += 1,
            Branch::NarrowNoHev => self.narrow_no_hev += 1,
            Branch::Wide8 => self.wide8 += 1,
            Branch::Wide14 => self.wide14 += 1,
        }
    }
    fn add(&mut self, o: Hits) {
        self.skipped += o.skipped;
        self.narrow_hev += o.narrow_hev;
        self.narrow_no_hev += o.narrow_no_hev;
        self.wide8 += o.wide8;
        self.wide14 += o.wide14;
    }
}

/// Determines which spec branch one line takes, independently of the kernels.
fn classify(edge: &Av1Edge4, lane: usize, prm: &Av1LfParams) -> Branch {
    let s = |k: usize| i32::from(edge[k][lane]);
    let (p0, p1, p2, p3) = (s(6), s(5), s(4), s(3));
    let (q0, q1, q2, q3) = (s(7), s(8), s(9), s(10));
    let flen = prm.filter_len();

    let mut mask = (p1 - p0).abs() > prm.limit;
    mask |= (q1 - q0).abs() > prm.limit;
    mask |= (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > prm.blimit;
    if flen >= 6 {
        mask |= (p2 - p1).abs() > prm.limit;
        mask |= (q2 - q1).abs() > prm.limit;
    }
    if flen >= 8 {
        mask |= (p3 - p2).abs() > prm.limit;
        mask |= (q3 - q2).abs() > prm.limit;
    }
    if mask {
        return Branch::Skipped;
    }

    let hev = (p1 - p0).abs() > prm.thresh || (q1 - q0).abs() > prm.thresh;
    let big = prm.size != Av1FilterSize::Size4;
    let widest = prm.size == Av1FilterSize::Size16;

    let flat = if big {
        let mut m = (p1 - p0).abs() > 1;
        m |= (q1 - q0).abs() > 1;
        m |= (p2 - p0).abs() > 1;
        m |= (q2 - q0).abs() > 1;
        if flen >= 8 {
            m |= (p3 - p0).abs() > 1;
            m |= (q3 - q0).abs() > 1;
        }
        !m
    } else {
        false
    };
    let flat2 = if widest {
        let (p4, p5, p6) = (s(2), s(1), s(0));
        let (q4, q5, q6) = (s(11), s(12), s(13));
        let mut m = (p6 - p0).abs() > 1;
        m |= (q6 - q0).abs() > 1;
        m |= (p5 - p0).abs() > 1;
        m |= (q5 - q0).abs() > 1;
        m |= (p4 - p0).abs() > 1;
        m |= (q4 - q0).abs() > 1;
        !m
    } else {
        false
    };

    if !big || !flat {
        if hev {
            Branch::NarrowHev
        } else {
            Branch::NarrowNoHev
        }
    } else if !widest || !flat2 {
        Branch::Wide8
    } else {
        Branch::Wide14
    }
}

// ── Sample generators ────────────────────────────────────────────────────────

/// Uniform random bytes — mostly reaches the `Skipped` branch.
fn gen_random(rng: &mut Rng) -> Av1Edge4 {
    let mut e = [[0u8; 4]; 14];
    for k in 0..14 {
        for lane in 0..4 {
            e[k][lane] = rng.byte();
        }
    }
    e
}

/// A step edge with per-sample jitter bounded by `limit`, so `filter_mask`
/// passes and `hev` fires or not depending on `thresh`.
fn gen_near_edge(rng: &mut Rng, prm: &Av1LfParams, jit: i32) -> Av1Edge4 {
    let mut e = [[0u8; 4]; 14];
    for lane in 0..4 {
        let base = 40 + rng.below(160) as i32;
        // Keep the step within what blimit admits.
        let max_step = (prm.blimit / 2).max(1);
        let step = rng.jitter(max_step);
        for k in 0..14 {
            let side = if k <= 6 { base } else { base + step };
            e[k][lane] = clamp_u8(side + rng.jitter(jit));
        }
    }
    e
}

/// Flat on both sides of a step: every p-sample within ±1 of `a`, every
/// q-sample within ±1 of `b`.  This is the case the wide filters exist for.
fn gen_flat_step(rng: &mut Rng, prm: &Av1LfParams) -> Av1Edge4 {
    let mut e = [[0u8; 4]; 14];
    for lane in 0..4 {
        let a = 40 + rng.below(160) as i32;
        let max_step = (prm.blimit / 2).max(1);
        let b = a + rng.jitter(max_step.min(40));
        for k in 0..14 {
            let side = if k <= 6 { a } else { b };
            e[k][lane] = clamp_u8(side + rng.jitter(1));
        }
    }
    e
}

/// Flat on the near samples but deliberately *not* flat further out, to hit
/// `flat && !flat2` (the 8-tap wide filter at `filterSize == 16`).
fn gen_flat_inner_only(rng: &mut Rng, prm: &Av1LfParams) -> Av1Edge4 {
    let mut e = gen_flat_step(rng, prm);
    for lane in 0..4 {
        // p4..p6 and q4..q6 far from p0/q0 → flat2 false, flat unaffected.
        for &k in &[0usize, 1, 2, 11, 12, 13] {
            e[k][lane] = if lane % 2 == 0 { 0 } else { 255 };
        }
    }
    e
}

/// A deliberately lane-divergent edge: each of the four lanes is built to a
/// different pattern so the batched kernel must blend, not broadcast.
fn gen_divergent(rng: &mut Rng, prm: &Av1LfParams) -> Av1Edge4 {
    let flat = gen_flat_step(rng, prm);
    let inner = gen_flat_inner_only(rng, prm);
    let near = gen_near_edge(rng, prm, prm.limit.max(1));
    let rand = gen_random(rng);
    let mut e = [[0u8; 4]; 14];
    for k in 0..14 {
        e[k] = [flat[k][0], inner[k][1], near[k][2], rand[k][3]];
    }
    e
}

// ── Core comparison ──────────────────────────────────────────────────────────

/// Runs every implementation on `edge` and asserts byte-identical results,
/// returning the branch histogram of the four lanes.
#[track_caller]
fn compare_all(edge: &Av1Edge4, prm: &Av1LfParams, what: &str) -> Hits {
    let mut hits = Hits::default();
    for lane in 0..4 {
        hits.record(classify(edge, lane, prm));
    }

    let mut want = *edge;
    super::filter_edge4_reference(&mut want, prm);

    let mut got_scalar = *edge;
    super::filter_edge4_scalar(&mut got_scalar, prm);
    assert_eq!(
        got_scalar, want,
        "scalar != reference ({what}, params {prm:?}, input {edge:?})"
    );

    let mut got_dispatch = *edge;
    super::filter_edge4(&mut got_dispatch, prm);
    assert_eq!(
        got_dispatch, want,
        "dispatched SIMD != reference ({what}, params {prm:?}, input {edge:?})"
    );

    // Samples outside the declared span must never be touched.
    for k in 0..14 {
        if !prm.span().contains(&k) {
            assert_eq!(
                got_dispatch[k], edge[k],
                "kernel wrote outside span at k={k} ({what}, params {prm:?})"
            );
        }
    }
    hits
}

fn all_configs() -> Vec<(Av1FilterSize, bool)> {
    vec![
        (Av1FilterSize::Size4, false),
        (Av1FilterSize::Size4, true),
        (Av1FilterSize::Size8, false),
        (Av1FilterSize::Size8, true),
        (Av1FilterSize::Size16, false),
        (Av1FilterSize::Size16, true),
    ]
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Exhaustive over every reachable `(lvl, sharpness)` — i.e. all filter
/// levels and thresholds — crossed with all block/filter sizes and both
/// planes, with generators biased to reach every filter branch.
#[test]
fn exhaustive_levels_and_sharpness_match_reference() {
    let mut rng = Rng::new(0x5EED_1234_ABCD_0001);
    let mut total = Hits::default();
    let mut cases = 0u64;

    for (size, chroma) in all_configs() {
        let mut per_config = Hits::default();
        for lvl in 0..=63i32 {
            for sharpness in 0..=7i32 {
                let (limit, blimit, thresh) = strength(lvl, sharpness);
                let prm = Av1LfParams {
                    limit,
                    blimit,
                    thresh,
                    size,
                    chroma,
                };
                let samples = [
                    gen_random(&mut rng),
                    gen_near_edge(&mut rng, &prm, limit),
                    gen_near_edge(&mut rng, &prm, 1),
                    gen_flat_step(&mut rng, &prm),
                    gen_flat_inner_only(&mut rng, &prm),
                    gen_divergent(&mut rng, &prm),
                ];
                for (i, e) in samples.iter().enumerate() {
                    per_config.add(compare_all(e, &prm, &format!("gen{i}")));
                    cases += 1;
                }
            }
        }
        total.add(per_config);

        // Coverage assertions: a sweep that never reaches a branch proves
        // nothing about that branch.
        assert!(
            per_config.skipped > 0,
            "{size:?} chroma={chroma}: never hit the skipped branch ({per_config:?})"
        );
        assert!(
            per_config.narrow_hev > 0,
            "{size:?} chroma={chroma}: never hit narrow+hev ({per_config:?})"
        );
        assert!(
            per_config.narrow_no_hev > 0,
            "{size:?} chroma={chroma}: never hit narrow-without-hev ({per_config:?})"
        );
        if size != Av1FilterSize::Size4 {
            assert!(
                per_config.wide8 > 0,
                "{size:?} chroma={chroma}: never hit the 6/8-tap wide filter ({per_config:?})"
            );
        }
        if size == Av1FilterSize::Size16 && !chroma {
            assert!(
                per_config.wide14 > 0,
                "{size:?} chroma={chroma}: never hit the 14-tap wide filter ({per_config:?})"
            );
        }
    }

    assert_eq!(cases, 6 * 64 * 8 * 6, "unexpected case count");
    assert!(
        total.wide14 > 0 && total.wide8 > 0,
        "wide filters never exercised: {total:?}"
    );
}

/// Randomised sweep over arbitrary (not necessarily reachable) parameter
/// triples, including degenerate ones, to make sure nothing depends on the
/// spec's parameter derivation holding.
#[test]
fn randomised_arbitrary_params_match_reference() {
    let mut rng = Rng::new(0xC0FF_EE00_1234_5678);
    let mut total = Hits::default();
    for _ in 0..4000 {
        let prm = Av1LfParams {
            limit: rng.below(64) as i32,
            blimit: rng.below(256) as i32,
            thresh: rng.below(32) as i32,
            size: match rng.below(3) {
                0 => Av1FilterSize::Size4,
                1 => Av1FilterSize::Size8,
                _ => Av1FilterSize::Size16,
            },
            chroma: rng.below(2) == 1,
        };
        let e = match rng.below(5) {
            0 => gen_random(&mut rng),
            1 => gen_near_edge(&mut rng, &prm, prm.limit.max(1)),
            2 => gen_flat_step(&mut rng, &prm),
            3 => gen_flat_inner_only(&mut rng, &prm),
            _ => gen_divergent(&mut rng, &prm),
        };
        total.add(compare_all(&e, &prm, "random-params"));
    }
    assert!(total.wide8 > 0 && total.wide14 > 0, "coverage: {total:?}");
}

/// Extremes of the sample range: the filters must stay bit-exact when the
/// clamps in the narrow filter and the edge clamp in the wide filter are
/// saturating.
#[test]
fn saturating_extremes_match_reference() {
    let mut total = Hits::default();
    for (size, chroma) in all_configs() {
        for &(limit, blimit, thresh) in &[
            (1i32, 5i32, 0i32),
            (9, 133, 3),
            (63, 260, 15),
            (0, 0, 0),
            (255, 1023, 63),
        ] {
            let prm = Av1LfParams {
                limit,
                blimit,
                thresh,
                size,
                chroma,
            };
            for &(a, b) in &[
                (0u8, 0u8),
                (255, 255),
                (0, 255),
                (255, 0),
                (0, 1),
                (254, 255),
                (127, 128),
                (1, 0),
            ] {
                let mut e = [[0u8; 4]; 14];
                for k in 0..14 {
                    for lane in 0..4 {
                        // Lane 3 gets the sides swapped so the batch is
                        // never uniform.
                        let v = if (k <= 6) == (lane != 3) { a } else { b };
                        e[k][lane] = v;
                    }
                }
                total.add(compare_all(&e, &prm, "extremes"));
            }
        }
    }
    assert!(total.wide8 > 0, "coverage: {total:?}");
}

/// The scalar fallback must stay compiled and must itself equal the spec
/// reference — this is the "force scalar" path test, independent of whatever
/// backend runtime dispatch would pick.
#[test]
fn forced_scalar_path_matches_reference() {
    let mut rng = Rng::new(0x1111_2222_3333_4444);
    let mut hits = Hits::default();
    for (size, chroma) in all_configs() {
        for lvl in (0..=63i32).step_by(3) {
            for sharpness in 0..=7i32 {
                let (limit, blimit, thresh) = strength(lvl, sharpness);
                let prm = Av1LfParams {
                    limit,
                    blimit,
                    thresh,
                    size,
                    chroma,
                };
                for e in [
                    gen_random(&mut rng),
                    gen_flat_step(&mut rng, &prm),
                    gen_flat_inner_only(&mut rng, &prm),
                    gen_divergent(&mut rng, &prm),
                ] {
                    for lane in 0..4 {
                        hits.record(classify(&e, lane, &prm));
                    }
                    let mut want = e;
                    super::filter_edge4_reference(&mut want, &prm);
                    let mut got = e;
                    super::filter_edge4_scalar(&mut got, &prm);
                    assert_eq!(got, want, "forced scalar != reference, params {prm:?}");
                }
            }
        }
    }
    assert!(
        hits.wide8 > 0 && hits.wide14 > 0 && hits.narrow_hev > 0,
        "forced-scalar coverage: {hits:?}"
    );
}

/// The batched kernel must equal four *independent* single-line filter runs.
/// This is the property that justifies putting four lines in four lanes.
#[test]
fn batch_of_four_equals_four_independent_lines() {
    let mut rng = Rng::new(0x9999_8888_7777_6666);
    for (size, chroma) in all_configs() {
        for lvl in (1..=63i32).step_by(5) {
            for sharpness in [0i32, 1, 5, 7] {
                let (limit, blimit, thresh) = strength(lvl, sharpness);
                let prm = Av1LfParams {
                    limit,
                    blimit,
                    thresh,
                    size,
                    chroma,
                };
                let e = gen_divergent(&mut rng, &prm);

                let mut batched = e;
                super::filter_edge4(&mut batched, &prm);

                // Filter each lane on its own, in a batch where the other
                // three lanes are copies of it, then take that lane back.
                let mut per_lane = e;
                for lane in 0..4 {
                    let mut solo = [[0u8; 4]; 14];
                    for k in 0..14 {
                        solo[k] = [e[k][lane]; 4];
                    }
                    super::filter_edge4_reference(&mut solo, &prm);
                    for k in 0..14 {
                        per_lane[k][lane] = solo[k][0];
                    }
                }
                assert_eq!(
                    batched, per_lane,
                    "batched kernel differs from independent per-line runs, params {prm:?}"
                );
            }
        }
    }
}

/// The sliding-window wide filter must equal the spec's O(n²) tap loop for
/// every `(log2Size, plane)` combination the spec defines, on flat inputs
/// where that branch is actually taken.
#[test]
fn wide_filter_specialisation_equals_spec_tap_loop() {
    let mut rng = Rng::new(0xABCD_0000_1234_9999);
    let mut seen = [0u64; 3]; // luma-8tap, chroma-6tap, luma-14tap
    for _ in 0..3000 {
        for (size, chroma) in all_configs() {
            let prm = Av1LfParams {
                limit: 8,
                blimit: 90,
                thresh: 2,
                size,
                chroma,
            };
            let e = gen_flat_step(&mut rng, &prm);
            for lane in 0..4 {
                match classify(&e, lane, &prm) {
                    Branch::Wide8 if !chroma => seen[0] += 1,
                    Branch::Wide8 => seen[1] += 1,
                    Branch::Wide14 => seen[2] += 1,
                    _ => {}
                }
            }
            let mut want = e;
            super::filter_edge4_reference(&mut want, &prm);
            let mut got = e;
            super::filter_edge4(&mut got, &prm);
            assert_eq!(got, want, "wide filter mismatch, params {prm:?}");
        }
    }
    assert!(
        seen[0] > 0 && seen[1] > 0 && seen[2] > 0,
        "wide-filter coverage per shape: {seen:?}"
    );
}

// ── Timing harness ───────────────────────────────────────────────────────────

/// Times the three implementations **interleaved within one process**, so a
/// loaded machine perturbs all of them alike, and reports the best round for
/// each.  Comparing separate builds is not reliable here; comparing rounds of
/// one build is.
#[test]
#[ignore = "timing harness; run with --release --ignored --nocapture"]
fn bench_edge_filter_variants() {
    use std::time::Instant;

    let mut rng = Rng::new(0xBEEF_0001_0002_0003);
    // A corpus per filter size whose branch mix is representative: most edges
    // fail the filter mask outright, some take the narrow filter, some are
    // flat enough for the wide filters.
    for (size, chroma, label) in [
        (Av1FilterSize::Size4, false, "size4  luma"),
        (Av1FilterSize::Size8, false, "size8  luma"),
        (Av1FilterSize::Size8, true, "size8  chroma"),
        (Av1FilterSize::Size16, false, "size16 luma"),
    ] {
        let mut corpus: Vec<(Av1Edge4, Av1LfParams)> = Vec::new();
        for n in 0..512 {
            let (limit, blimit, thresh) = strength(8 + (n % 40) as i32, (n % 8) as i32);
            let prm = Av1LfParams {
                limit,
                blimit,
                thresh,
                size,
                chroma,
            };
            let e = match n % 10 {
                0..=4 => gen_random(&mut rng),
                5..=7 => gen_near_edge(&mut rng, &prm, limit),
                8 => gen_flat_step(&mut rng, &prm),
                _ => gen_flat_inner_only(&mut rng, &prm),
            };
            corpus.push((e, prm));
        }
        let mut mix = Hits::default();
        for (e, p) in &corpus {
            for lane in 0..4 {
                mix.record(classify(e, lane, p));
            }
        }

        type Kernel = fn(&mut Av1Edge4, &Av1LfParams);
        let variants: [(&str, Kernel); 3] = [
            ("spec-reference", super::filter_edge4_reference),
            ("portable-scalar", super::filter_edge4_scalar),
            ("dispatched-simd", super::filter_edge4),
        ];
        let mut best = [f64::MAX; 3];
        let mut sink = 0u64;
        for _round in 0..30 {
            for (vi, (_, f)) in variants.iter().enumerate() {
                let t0 = Instant::now();
                for _ in 0..20 {
                    for (e, p) in &corpus {
                        let mut w = *e;
                        f(&mut w, p);
                        sink = sink.wrapping_add(u64::from(w[7][0]));
                    }
                }
                let dt = t0.elapsed().as_secs_f64();
                if dt < best[vi] {
                    best[vi] = dt;
                }
            }
        }
        let ns = |t: f64| t * 1e9 / (20.0 * 512.0);
        println!(
            "lf {label}: reference {:7.2} ns/edge | scalar {:7.2} ({:.2}x) | simd {:7.2} ({:.2}x)  [mix {mix:?}, sink {sink}]",
            ns(best[0]),
            ns(best[1]),
            best[0] / best[1],
            ns(best[2]),
            best[0] / best[2],
        );
    }
}
