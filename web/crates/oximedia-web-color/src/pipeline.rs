// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! The fused colour-science pipeline.
//!
//! # Fixed operator order
//!
//! ```text
//! encoded input
//!   │ 1. input transfer decode  (sRGB / PQ / HLG / linear → linear f32)
//!   │ 2. exposure               (× 2^stops, in linear light)
//!   │ 3. contrast               (power law around the 0.18 linear pivot)
//!   │ 4. saturation             (BT.709 luma blend, in linear light)
//!   │ 5. tone map               (optional; Reinhard / Hable / ACES / ACES-ODT)
//!   │ 6. gamut conversion       (optional; 3×3 primaries matrix + OOG fix-up)
//!   │ 7. output transfer encode (linear → sRGB / PQ / HLG / linear)
//!   │ 8. 3D LUT                 (optional; applied on ENCODED values, the
//!   │                            standard creative-LUT convention)
//!   ▼ encoded output
//! ```
//!
//! Alpha passes through untouched. The whole chain is fused per pixel: after
//! construction/configuration the `apply_*` methods allocate nothing.
//!
//! # Data plane
//!
//! * [`ColorPipeline::apply_rgba8`] — 8-bit SDR. By default this runs the
//!   **baked** fast path (the standard colorist-tool architecture): whenever
//!   the configuration changes, the whole chain is re-sampled into one
//!   internal 33³ fixed-point lattice (~36 K chain evaluations, amortised
//!   across frames), and per-frame work collapses to a single trilinear
//!   lattice interpolation per pixel. Baking quantises: output may differ
//!   from the exact chain by up to ~2/255 per channel for smooth operator
//!   chains; chains with hard clip surfaces (gamut fix-up, saturation-induced
//!   clipping) can deviate further inside the lattice cells the clip surface
//!   crosses (localised — ~1% of the RGB cube in a torture chain).
//!   Call [`ColorPipeline::set_exact`]`(true)` to opt out and run
//!   the exact per-pixel chain instead (decode via an exact 256-entry
//!   table; encode via a 4 096-interval sqrt-domain LUT, error ≪ half an
//!   8-bit code).
//! * [`ColorPipeline::apply_rgba_f32`] — HDR/linear float path using the
//!   exact transfer curves (no table quantisation, never baked).

use crate::error::ColorError;
use crate::gamut::{GamutMap, Primaries};
use crate::lut::{Lut3d, LutInterp, MAX_LUT_SIZE, MIN_LUT_SIZE};
use crate::tone_map::{ToneMap, ToneMapOperator};
use crate::transfer::{EncodeLut, Transfer};
use oximedia_web_core::normalize::u8_to_f32;
use oximedia_web_core::Scratch;

/// BT.709 luma coefficients (saturation op).
const B709_R: f32 = 0.2126;
const B709_G: f32 = 0.7152;
const B709_B: f32 = 0.0722;

/// Linear-light contrast pivot (photographic 18% grey).
const CONTRAST_PIVOT: f32 = 0.18;

/// Pixels per processing tile on the u8 path: 2 048 px × 16 B of f32
/// scratch = 32 KiB, small enough to stay cache-resident while amortising
/// stage dispatch (each stage runs as a tight branch-hoisted sweep over the
/// tile instead of per-pixel dispatch).
const TILE_PIXELS: usize = 2048;

/// Lattice size of the internal baked chain (default u8 fast path): the
/// standard colorist-tool 33³ resolution — dense enough that trilinear error
/// vs the exact chain stays within ~2/255 for smooth operator chains, small
/// enough (≈36 K chain evaluations, 287 KiB of u16 lattice) that a rebake
/// after a config change amortises to nothing across frames.
const BAKE_SIZE: usize = 33;

/// 8.8 fixed-point scale of the baked lattice: encoded `[0, 1]` maps to
/// `0..=255 × 256` (65 280, fits `u16`).
const BAKED_SCALE: f32 = 65280.0;

/// Total lattice points. Each point packs its three u16 channels into one
/// `u64` (R in bits 0..16, G in 16..32, B in 32..48, top 16 bits zero):
/// a corner fetch is then exactly **one** 8-byte load plus register
/// shifts, versus three separate stride-3 u16 loads — the sampler is
/// load-bound, and on wasm the difference is one `i64.load` instruction
/// per corner. Costs +33% lattice memory vs packed u16×3 (287 KiB total,
/// still L2-resident).
const BAKE_LEN: usize = BAKE_SIZE * BAKE_SIZE * BAKE_SIZE;

/// Mask that bounds a lattice cell index to `0..=BAKE_SIZE-2`. A single
/// `AND` gives the compiler a provable range, which (with the fixed-size
/// `data` array below) lets it drop every per-pixel bounds check in
/// [`BakedChain::sample`].
const CELL_MASK: u8 = 31;
const _: () = assert!(BAKE_SIZE - 2 == CELL_MASK as usize, "mask must match BAKE_SIZE");

/// Unpacks one lattice point (see [`BAKE_LEN`] for the packing). The
/// masked cell indices plus the fixed-size array type make every index
/// provably in bounds, so the compiler drops the checks on the hot path.
#[inline]
#[allow(clippy::cast_possible_truncation)]
fn corner(data: &[u64; BAKE_LEN + 1], i: usize) -> [i32; 3] {
    let v = data[i];
    [
        (v & 0xFFFF) as i32,
        ((v >> 16) & 0xFFFF) as i32,
        ((v >> 32) & 0xFFFF) as i32,
    ]
}

/// The whole configured chain baked into one fixed-point 33³ lattice.
///
/// `data` follows the same R-fastest layout as [`Lut3d`] with one packed
/// `u64` per lattice point (see [`BAKE_LEN`]) plus one pad point; the
/// packed channels are the encoded output scaled by [`BAKED_SCALE`]
/// (8.8 fixed point of the 0..=255 output code). `idx`/`frac` are the
/// precomputed u8 → lattice locate tables (`frac` is a 0..=256 cell
/// fraction), shared by all three axes.
#[derive(Debug)]
struct BakedChain {
    data: Box<[u64; BAKE_LEN + 1]>,
    idx: [u8; 256],
    frac: [i32; 256],
}

impl BakedChain {
    /// Fixed-point Sakamoto **tetrahedral** sample (the film-industry
    /// standard for LUT application): one encoded RGB byte triplet in, one
    /// out. Four corner fetches and nine integer multiplies per pixel —
    /// roughly half the memory traffic and arithmetic of 8-corner
    /// trilinear — with a single rounding at the end (the barycentric sum
    /// accumulates at 16.8 precision before the final shift). Exact for
    /// per-axis-linear lattices, so an identity bake reproduces input
    /// bytes exactly.
    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    fn sample(&self, r: u8, g: u8, b: u8) -> [u8; 3] {
        /// Point strides of one lattice step along each axis.
        const DR: usize = 1;
        const DG: usize = BAKE_SIZE;
        const DB: usize = BAKE_SIZE * BAKE_SIZE;
        let r0 = usize::from(self.idx[r as usize] & CELL_MASK);
        let g0 = usize::from(self.idx[g as usize] & CELL_MASK);
        let b0 = usize::from(self.idx[b as usize] & CELL_MASK);
        let rf = self.frac[r as usize];
        let gf = self.frac[g as usize];
        let bf = self.frac[b as usize];
        let base = (b0 * BAKE_SIZE + g0) * BAKE_SIZE + r0;

        // Sakamoto decomposition, **branchlessly**: sort the three
        // fractions to pick the tetrahedron; the interpolation path steps
        // one axis at a time from c000 to c111 in descending-fraction
        // order. The i1/i2 corner offsets come from an 8-entry table
        // indexed by the three comparison bits (comparisons become
        // selects/table loads instead of an unpredictable 6-way branch
        // tree — per-pixel branch outcomes are data-dependent and noisy
        // frames mispredict them), and the sorted weights come from a
        // min/max network. Case-by-case this reproduces the classic
        // decision tree exactly, ties included:
        //
        //   rf>=gf, gf>=bf          -> c100, c110  w=(rf,gf,bf)
        //   rf>=gf, gf<bf,  rf>=bf  -> c100, c101  w=(rf,bf,gf)
        //   rf>=gf, gf<bf,  rf<bf   -> c001, c101  w=(bf,rf,gf)
        //   rf<gf,  bf>=gf          -> c001, c011  w=(bf,gf,rf)
        //   rf<gf,  bf<gf,  bf>rf   -> c010, c011  w=(gf,bf,rf)
        //   rf<gf,  bf<gf,  bf<=rf  -> c010, c110  w=(gf,rf,bf)
        //
        // In every case w1/w2/w3 are the descending-sorted fractions, so
        // `w1 = max`, `w3 = min`, `w2 = sum - max - min` — exact in i32.
        const TAB: [(usize, usize); 8] = [
            (DG, DR + DG), // 000: rf<gf,  bf<gf, bf<=rf -> c010, c110
            (DG, DG + DB), // 001: rf<gf,  bf<gf, bf>rf  -> c010, c011
            (DB, DG + DB), // 010: rf<gf,  bf>=gf        -> c001, c011
            (DB, DG + DB), // 011: (rf<gf, bf>=gf; bit 0 irrelevant)
            (DB, DR + DB), // 100: rf>=gf, gf<bf, rf<bf  -> c001, c101
            (DR, DR + DB), // 101: rf>=gf, gf<bf, rf>=bf -> c100, c101
            (DR, DR + DG), // 110: rf>=gf, gf>=bf        -> c100, c110
            (DR, DR + DG), // 111: (rf>=gf, gf>=bf; bit 0 irrelevant)
        ];
        // Both candidate bit pairs are computed eagerly so the `if` lowers
        // to a select rather than a data-dependent branch.
        let hi_bits = (usize::from(gf >= bf) << 1) | usize::from(rf >= bf);
        let lo_bits = (usize::from(bf >= gf) << 1) | usize::from(bf > rf);
        let sel = if rf >= gf { 4 | hi_bits } else { lo_bits };
        let (o1, o2) = TAB[sel];
        let (i1, i2) = (base + o1, base + o2);
        let hi_rg = rf.max(gf);
        let lo_rg = rf.min(gf);
        let w1 = hi_rg.max(bf);
        let w3 = lo_rg.min(bf);
        let w2 = rf + gf + bf - w1 - w3;

        let v0 = corner(&self.data, base);
        let v1 = corner(&self.data, i1);
        let v2 = corner(&self.data, i2);
        let v3 = corner(&self.data, base + DR + DG + DB);

        // No final clamp needed: the barycentric expansion is
        // `(256-w1)·v0 + (w1-w2)·v1 + (w2-w3)·v2 + w3·v3` whose four
        // coefficients are each >= 0 (the decision tree above always yields
        // `w1 >= w2 >= w3`, all in 0..=256) and sum to exactly 256, and every
        // lattice entry is <= 65 280 — so `acc <= 65 280 * 256` and
        // `(acc + 32768) >> 16 <= 255` unconditionally.
        let mut out = [0u8; 3];
        for c in 0..3 {
            let acc = (v0[c] << 8)
                + w1 * (v1[c] - v0[c])
                + w2 * (v2[c] - v1[c])
                + w3 * (v3[c] - v2[c]);
            out[c] = ((acc + 32768) >> 16) as u8;
        }
        out
    }
}

/// Packs a pixel's RGB bytes plus a constant marker byte into the memo key
/// used by the baked sweeps below (one 32-bit compare per pixel). The
/// marker can never appear in a fresh sentinel, so the first pixel of a
/// sweep always samples.
#[inline]
fn memo_key(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16) | (0x01 << 24)
}

/// Sentinel [`memo_key`] value no packed pixel can produce.
const MEMO_NONE: u32 = 0xFFFF_FFFF;

/// Baked-chain sweep over a src/dst RGBA8 pair (alpha copied through).
///
/// Consecutive identical RGB triplets reuse the previous sample (one u32
/// compare) — flat regions and smooth gradients repeat pixels in runs, so
/// this cuts the gradient-frame sweep ~2.7x while costing noise-frame
/// sweeps nothing measurable. Output is byte-identical either way.
fn apply_baked(baked: &BakedChain, src: &[u8], dst: &mut [u8]) {
    let mut last_key = MEMO_NONE;
    let mut last_out = [0u8; 3];
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let key = memo_key(s[0], s[1], s[2]);
        if key != last_key {
            last_out = baked.sample(s[0], s[1], s[2]);
            last_key = key;
        }
        d[0] = last_out[0];
        d[1] = last_out[1];
        d[2] = last_out[2];
        d[3] = s[3];
    }
}

/// In-place baked-chain sweep (alpha bytes untouched); same last-pixel
/// memo as [`apply_baked`].
fn apply_baked_in_place(baked: &BakedChain, buf: &mut [u8]) {
    let mut last_key = MEMO_NONE;
    let mut last_out = [0u8; 3];
    for px in buf.chunks_exact_mut(4) {
        let key = memo_key(px[0], px[1], px[2]);
        if key != last_key {
            last_out = baked.sample(px[0], px[1], px[2]);
            last_key = key;
        }
        px[0] = last_out[0];
        px[1] = last_out[1];
        px[2] = last_out[2];
    }
}

/// Saturating round-half-up quantiser. Bit-identical to
/// `oximedia_web_core::normalize::f32_to_u8` for every input (positive
/// round-half-away equals `+0.5` + truncation; NaN saturates to 0), but the
/// `+0.5` + saturating-cast form lowers to `i32.trunc_sat_f32_u` on wasm
/// instead of a libm `roundf` call.
#[inline]
fn quantize_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Contrast op: power law around the 0.18 linear pivot — monotonic and
/// pivot-preserving. Negative linear light (possible with out-of-gamut
/// input) clamps to 0.
#[inline]
fn apply_contrast(v: f32, contrast: f32) -> f32 {
    if v <= 0.0 {
        0.0
    } else {
        CONTRAST_PIVOT * (v / CONTRAST_PIVOT).powf(contrast)
    }
}

/// Configured LUT stage.
#[derive(Clone, Debug)]
struct LutStage {
    lut: Lut3d,
    interp: LutInterp,
}

/// The fused colour pipeline. See the [module docs](self) for the operator
/// order and data-plane guarantees.
#[derive(Debug)]
pub struct ColorPipeline {
    exposure_gain: f32,
    contrast: f32,
    saturation: f32,
    tone_map: Option<ToneMap>,
    gamut: Option<GamutMap>,
    lut: Option<LutStage>,
    input_transfer: Transfer,
    output_transfer: Transfer,
    /// Exact per-code decode table for the u8 path.
    decode_table: [f32; 256],
    /// Sqrt-domain encode LUT for the u8 path (`None` = linear output).
    encode_lut: Option<EncodeLut>,
    /// Grow-once tile buffer for the u8 path (no per-frame allocation).
    scratch: Scratch,
    /// `true` = the u8 path runs the exact per-pixel chain instead of the
    /// baked lattice (see [`ColorPipeline::set_exact`]).
    exact: bool,
    /// The baked chain, rebuilt lazily on the first u8 apply after a config
    /// change (`None` = dirty).
    baked: Option<BakedChain>,
    /// `true` when the current configuration failed to bake (non-finite
    /// output, possible only with a linear output transfer on extreme
    /// settings) — the u8 path then falls back to the exact chain instead
    /// of re-attempting the bake every frame. Reset by any config change.
    bake_failed: bool,
}

impl Default for ColorPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorPipeline {
    /// Creates an identity pipeline (sRGB in, sRGB out, all ops neutral).
    #[must_use]
    pub fn new() -> Self {
        let input_transfer = Transfer::Srgb;
        let output_transfer = Transfer::Srgb;
        Self {
            exposure_gain: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            tone_map: None,
            gamut: None,
            lut: None,
            input_transfer,
            output_transfer,
            decode_table: Self::build_decode_table(input_transfer),
            encode_lut: EncodeLut::build(output_transfer),
            scratch: Scratch::with_capacity(0, TILE_PIXELS * 4),
            exact: false,
            baked: None,
            bake_failed: false,
        }
    }

    fn build_decode_table(transfer: Transfer) -> [f32; 256] {
        core::array::from_fn(|i| transfer.decode(u8_to_f32(i as u8)))
    }

    /// Marks the baked chain stale after any configuration change.
    #[inline]
    fn mark_dirty(&mut self) {
        self.baked = None;
        self.bake_failed = false;
    }

    /// Samples the full chain into the internal fixed-point lattice.
    /// Returns `None` if the chain produces non-finite output anywhere.
    fn bake_chain(&self) -> Option<BakedChain> {
        let size = BAKE_SIZE;
        #[allow(clippy::cast_precision_loss)]
        let inv = 1.0 / (size - 1) as f32;
        let mut data = Vec::with_capacity(BAKE_LEN + 1);
        for b in 0..size {
            #[allow(clippy::cast_precision_loss)]
            let fb = b as f32 * inv;
            for g in 0..size {
                #[allow(clippy::cast_precision_loss)]
                let fg = g as f32 * inv;
                for r in 0..size {
                    #[allow(clippy::cast_precision_loss)]
                    let out = self.sample_encoded(r as f32 * inv, fg, fb);
                    let mut packed = 0u64;
                    for (c, v) in out.into_iter().enumerate() {
                        if !v.is_finite() {
                            return None;
                        }
                        #[allow(clippy::cast_possible_truncation)]
                        #[allow(clippy::cast_sign_loss)]
                        let q = (v.clamp(0.0, 1.0) * BAKED_SCALE + 0.5) as u16;
                        packed |= u64::from(q) << (16 * c);
                    }
                    data.push(packed);
                }
            }
        }
        data.push(0); // pad point (see `corner`)
        let data: Box<[u64; BAKE_LEN + 1]> = data.into_boxed_slice().try_into().ok()?;

        // u8 -> lattice locate tables: `pos = round(v·(size−1)·256 / 255)`
        // in 8.8 fixed point, computed exactly in integers. `frac` may be
        // 256 when a byte lands exactly on the next lattice plane (v = 255).
        let mut idx = [0u8; 256];
        let mut frac = [0i32; 256];
        for v in 0..256usize {
            let pos = (v * (size - 1) * 512 + 255) / 510;
            let cell = (pos >> 8).min(size - 2);
            #[allow(clippy::cast_possible_truncation)]
            {
                idx[v] = cell as u8;
            }
            #[allow(clippy::cast_possible_wrap)]
            {
                frac[v] = (pos - (cell << 8)) as i32;
            }
        }
        Some(BakedChain { data, idx, frac })
    }

    /// Rebakes the lattice if the configuration changed since the last u8
    /// apply. After this, `self.baked` is `Some` unless the chain cannot be
    /// baked (non-finite output), in which case `bake_failed` latches and
    /// the caller falls back to the exact path.
    fn ensure_baked(&mut self) {
        if self.baked.is_none() && !self.bake_failed {
            self.baked = self.bake_chain();
            self.bake_failed = self.baked.is_none();
        }
    }

    // ── Configuration (never on the per-frame hot path) ─────────────────────

    /// Sets exposure in photographic stops (`gain = 2^stops`, applied in
    /// linear light). 0.0 is neutral.
    ///
    /// # Errors
    /// [`ColorError::NonFinite`] for NaN/infinite input;
    /// [`ColorError::OutOfRange`] for |stops| > 32.
    pub fn set_exposure(&mut self, stops: f32) -> Result<(), ColorError> {
        if !stops.is_finite() {
            return Err(ColorError::NonFinite { what: "exposure stops" });
        }
        if stops.abs() > 32.0 {
            return Err(ColorError::OutOfRange { what: "exposure stops" });
        }
        self.exposure_gain = 2.0f32.powf(stops);
        self.mark_dirty();
        Ok(())
    }

    /// Sets contrast (power law around the 0.18 linear pivot). 1.0 is
    /// neutral; valid range `(0, 10]`.
    ///
    /// # Errors
    /// [`ColorError::NonFinite`] / [`ColorError::OutOfRange`] on bad input.
    pub fn set_contrast(&mut self, contrast: f32) -> Result<(), ColorError> {
        if !contrast.is_finite() {
            return Err(ColorError::NonFinite { what: "contrast" });
        }
        if contrast <= 0.0 || contrast > 10.0 {
            return Err(ColorError::OutOfRange { what: "contrast" });
        }
        self.contrast = contrast;
        self.mark_dirty();
        Ok(())
    }

    /// Sets saturation (BT.709 luma blend in linear light). 1.0 is neutral,
    /// 0.0 is monochrome; valid range `[0, 10]`.
    ///
    /// # Errors
    /// [`ColorError::NonFinite`] / [`ColorError::OutOfRange`] on bad input.
    pub fn set_saturation(&mut self, saturation: f32) -> Result<(), ColorError> {
        if !saturation.is_finite() {
            return Err(ColorError::NonFinite { what: "saturation" });
        }
        if !(0.0..=10.0).contains(&saturation) {
            return Err(ColorError::OutOfRange { what: "saturation" });
        }
        self.saturation = saturation;
        self.mark_dirty();
        Ok(())
    }

    /// Enables the tone-map stage.
    ///
    /// # Errors
    /// Propagates [`ToneMap::new`] validation.
    pub fn set_tone_map(
        &mut self,
        op: ToneMapOperator,
        input_peak_nits: f32,
        output_peak_nits: f32,
    ) -> Result<(), ColorError> {
        self.tone_map = Some(ToneMap::new(op, input_peak_nits, output_peak_nits)?);
        self.mark_dirty();
        Ok(())
    }

    /// Disables the tone-map stage.
    pub fn clear_tone_map(&mut self) {
        self.tone_map = None;
        self.mark_dirty();
    }

    /// Enables the gamut-conversion stage (default softness 0 — negative
    /// channels are fixed hue-preservingly, HDR values above 1.0 survive).
    ///
    /// # Errors
    /// Propagates [`GamutMap::new`] validation.
    pub fn set_gamut(&mut self, src: Primaries, dst: Primaries) -> Result<(), ColorError> {
        self.gamut = Some(GamutMap::new(src, dst)?);
        self.mark_dirty();
        Ok(())
    }

    /// Adjusts the soft-clip softness of the active gamut stage
    /// (see [`GamutMap::set_softness`]).
    ///
    /// # Errors
    /// [`ColorError::OutOfRange`] if no gamut stage is configured, plus
    /// [`GamutMap::set_softness`] validation.
    pub fn set_gamut_softness(&mut self, softness: f32) -> Result<(), ColorError> {
        let result = match self.gamut.as_mut() {
            Some(g) => g.set_softness(softness),
            None => Err(ColorError::OutOfRange {
                what: "gamut softness (no gamut stage configured)",
            }),
        };
        if result.is_ok() {
            self.mark_dirty();
        }
        result
    }

    /// Disables the gamut stage.
    pub fn clear_gamut(&mut self) {
        self.gamut = None;
        self.mark_dirty();
    }

    /// Enables the 3D-LUT stage (applied on encoded output values).
    pub fn set_lut(&mut self, lut: Lut3d, interp: LutInterp) {
        self.lut = Some(LutStage { lut, interp });
        self.mark_dirty();
    }

    /// Disables the 3D-LUT stage.
    pub fn clear_lut(&mut self) {
        self.lut = None;
        self.mark_dirty();
    }

    /// Sets the input transfer function (rebuilds the u8 decode table).
    pub fn set_input_transfer(&mut self, transfer: Transfer) {
        self.input_transfer = transfer;
        self.decode_table = Self::build_decode_table(transfer);
        self.mark_dirty();
    }

    /// Sets the output transfer function (rebuilds the u8 encode LUT).
    pub fn set_output_transfer(&mut self, transfer: Transfer) {
        self.output_transfer = transfer;
        self.encode_lut = EncodeLut::build(transfer);
        self.mark_dirty();
    }

    /// Selects the u8 data plane: `false` (the default) runs the baked
    /// fast path — the whole chain sampled into one internal 33³ lattice,
    /// one trilinear interpolation per pixel per frame; `true` runs the
    /// exact per-pixel chain every frame (~2/255 more accurate, several
    /// times slower). [`ColorPipeline::apply_rgba_f32`] is always exact.
    pub fn set_exact(&mut self, exact: bool) {
        self.exact = exact;
    }

    /// Whether the u8 path runs the exact per-pixel chain
    /// (see [`ColorPipeline::set_exact`]).
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        self.exact
    }

    /// Current input transfer.
    #[must_use]
    pub const fn input_transfer(&self) -> Transfer {
        self.input_transfer
    }

    /// Current output transfer.
    #[must_use]
    pub const fn output_transfer(&self) -> Transfer {
        self.output_transfer
    }

    // ── Fused chain ──────────────────────────────────────────────────────────

    /// Ops 2–6 for a single pixel (exact path: [`Self::sample_encoded`],
    /// baking, tests). The slice sweeps in [`run_linear_stages`] use the
    /// same primitives, so both paths are bit-identical.
    #[inline]
    fn process_linear(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        // 2. exposure
        let gain = self.exposure_gain;
        let (mut r, mut g, mut b) = (r * gain, g * gain, b * gain);

        // 3. contrast
        if self.contrast != 1.0 {
            r = apply_contrast(r, self.contrast);
            g = apply_contrast(g, self.contrast);
            b = apply_contrast(b, self.contrast);
        }

        // 4. saturation (BT.709 luma blend)
        if self.saturation != 1.0 {
            let s = self.saturation;
            let luma = B709_R * r + B709_G * g + B709_B * b;
            r = luma + (r - luma) * s;
            g = luma + (g - luma) * s;
            b = luma + (b - luma) * s;
        }

        // 5. tone map
        if let Some(tm) = &self.tone_map {
            let (tr, tg, tb) = tm.map_rgb(r, g, b);
            r = tr;
            g = tg;
            b = tb;
        }

        // 6. gamut
        if let Some(gm) = &self.gamut {
            let (gr, gg, gb) = gm.convert(r, g, b);
            r = gr;
            g = gg;
            b = gb;
        }

        (r, g, b)
    }

    /// Full exact-precision chain for one encoded RGB triplet (used by the
    /// f32 path and by [`ColorPipeline::bake_lut`]).
    #[inline]
    #[must_use]
    pub fn sample_encoded(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let t_in = self.input_transfer;
        let (lr, lg, lb) =
            self.process_linear(t_in.decode(r), t_in.decode(g), t_in.decode(b));
        let t_out = self.output_transfer;
        let (mut er, mut eg, mut eb) = (t_out.encode(lr), t_out.encode(lg), t_out.encode(lb));
        if let Some(stage) = &self.lut {
            let out = stage.lut.sample(stage.interp, er, eg, eb);
            er = out[0];
            eg = out[1];
            eb = out[2];
        }
        [er, eg, eb]
    }

    // ── Frame application (no allocation after construction) ────────────────

    fn validate_pair(src_len: usize, dst_len: usize) -> Result<(), ColorError> {
        if src_len != dst_len {
            return Err(ColorError::LengthMismatch {
                left: src_len,
                right: dst_len,
            });
        }
        if !src_len.is_multiple_of(4) {
            return Err(ColorError::NotRgba { len: src_len });
        }
        Ok(())
    }

    /// Applies the pipeline to an interleaved RGBA8 buffer (tightly packed).
    ///
    /// `src` and `dst` must have equal lengths that are multiples of 4.
    /// Alpha is copied through untouched.
    ///
    /// By default this runs the **baked** fast path: the first call after a
    /// config change re-samples the chain into an internal 33³ lattice
    /// (amortised across frames), and every pixel is then one trilinear
    /// interpolation. Output may differ from the exact chain by ~2/255 per
    /// channel; call [`ColorPipeline::set_exact`]`(true)` for the exact
    /// per-pixel chain (tiled branch-hoisted sweeps, auto-vectorised under
    /// `+simd128`). Allocation-free after the first call in either mode.
    ///
    /// # Errors
    /// [`ColorError::LengthMismatch`] / [`ColorError::NotRgba`] on bad
    /// buffer geometry.
    pub fn apply_rgba8(&mut self, src: &[u8], dst: &mut [u8]) -> Result<(), ColorError> {
        Self::validate_pair(src.len(), dst.len())?;
        if !self.exact {
            self.ensure_baked();
            if let Some(baked) = &self.baked {
                apply_baked(baked, src, dst);
                return Ok(());
            }
            // Bake failed (non-finite chain output): honest fallback to the
            // exact path below.
        }
        let tile_len = TILE_PIXELS * 4;

        for (s_tile, d_tile) in src.chunks(tile_len).zip(dst.chunks_mut(tile_len)) {
            // NOTE: `self.scratch` is borrowed mutably while the other
            // fields are read — disjoint field borrows, no aliasing.
            let buf = self.scratch.floats(s_tile.len());

            // 1. decode (exact 256-entry table; alpha lane parked at 0).
            for (f, s) in buf.chunks_exact_mut(4).zip(s_tile.chunks_exact(4)) {
                f[0] = self.decode_table[s[0] as usize];
                f[1] = self.decode_table[s[1] as usize];
                f[2] = self.decode_table[s[2] as usize];
                f[3] = 0.0;
            }

            // 2–6. linear-light stages.
            run_linear_stages(
                buf,
                self.exposure_gain,
                self.contrast,
                self.saturation,
                self.tone_map.as_ref(),
                self.gamut.as_ref(),
            );

            // 7. encode (sqrt-domain LUT; `None` = linear passthrough).
            if let Some(enc) = &self.encode_lut {
                for f in buf.chunks_exact_mut(4) {
                    f[0] = enc.eval(f[0]);
                    f[1] = enc.eval(f[1]);
                    f[2] = enc.eval(f[2]);
                }
            }

            // 8. 3D LUT on encoded values.
            if let Some(stage) = &self.lut {
                apply_lut_slice(&stage.lut, stage.interp, buf);
            }

            // 9. quantise + alpha passthrough.
            for ((d, f), s) in d_tile
                .chunks_exact_mut(4)
                .zip(buf.chunks_exact(4))
                .zip(s_tile.chunks_exact(4))
            {
                d[0] = quantize_u8(f[0]);
                d[1] = quantize_u8(f[1]);
                d[2] = quantize_u8(f[2]);
                d[3] = s[3];
            }
        }
        Ok(())
    }

    /// In-place variant of [`ColorPipeline::apply_rgba8`]: the buffer is
    /// both source and destination (alpha bytes are simply left untouched).
    /// Runs the baked fast path unless [`ColorPipeline::set_exact`]`(true)`.
    ///
    /// This is the preferred wasm entry point — a single `&mut [u8]` halves
    /// the JS↔wasm boundary traffic versus separate src/dst slices.
    ///
    /// # Errors
    /// [`ColorError::NotRgba`] if the length is not a multiple of 4.
    pub fn apply_rgba8_in_place(&mut self, buf: &mut [u8]) -> Result<(), ColorError> {
        if !buf.len().is_multiple_of(4) {
            return Err(ColorError::NotRgba { len: buf.len() });
        }
        if !self.exact {
            self.ensure_baked();
            if let Some(baked) = &self.baked {
                apply_baked_in_place(baked, buf);
                return Ok(());
            }
        }
        let tile_len = TILE_PIXELS * 4;

        for tile in buf.chunks_mut(tile_len) {
            let fbuf = self.scratch.floats(tile.len());

            for (f, s) in fbuf.chunks_exact_mut(4).zip(tile.chunks_exact(4)) {
                f[0] = self.decode_table[s[0] as usize];
                f[1] = self.decode_table[s[1] as usize];
                f[2] = self.decode_table[s[2] as usize];
                f[3] = 0.0;
            }

            run_linear_stages(
                fbuf,
                self.exposure_gain,
                self.contrast,
                self.saturation,
                self.tone_map.as_ref(),
                self.gamut.as_ref(),
            );

            if let Some(enc) = &self.encode_lut {
                for f in fbuf.chunks_exact_mut(4) {
                    f[0] = enc.eval(f[0]);
                    f[1] = enc.eval(f[1]);
                    f[2] = enc.eval(f[2]);
                }
            }

            if let Some(stage) = &self.lut {
                apply_lut_slice(&stage.lut, stage.interp, fbuf);
            }

            for (d, f) in tile.chunks_exact_mut(4).zip(fbuf.chunks_exact(4)) {
                d[0] = quantize_u8(f[0]);
                d[1] = quantize_u8(f[1]);
                d[2] = quantize_u8(f[2]);
                // d[3] (alpha) untouched.
            }
        }
        Ok(())
    }

    /// Applies the pipeline to an interleaved RGBA `f32` buffer using the
    /// exact transfer curves (HDR path).
    ///
    /// `src` and `dst` must have equal lengths that are multiples of 4.
    /// Alpha is copied through untouched. Allocation-free (`dst` doubles as
    /// the working buffer).
    ///
    /// # Errors
    /// [`ColorError::LengthMismatch`] / [`ColorError::NotRgba`] on bad
    /// buffer geometry.
    pub fn apply_rgba_f32(&mut self, src: &[f32], dst: &mut [f32]) -> Result<(), ColorError> {
        Self::validate_pair(src.len(), dst.len())?;
        let t_in = self.input_transfer;
        let t_out = self.output_transfer;

        // 1. decode straight into dst (the in-place working buffer).
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[0] = t_in.decode(s[0]);
            d[1] = t_in.decode(s[1]);
            d[2] = t_in.decode(s[2]);
            d[3] = 0.0;
        }

        // 2–6. linear-light stages.
        run_linear_stages(
            dst,
            self.exposure_gain,
            self.contrast,
            self.saturation,
            self.tone_map.as_ref(),
            self.gamut.as_ref(),
        );

        // 7. encode in place (exact curves; `Transfer::Linear` squashes
        // non-finite values, matching `Transfer::encode` semantics).
        for d in dst.chunks_exact_mut(4) {
            d[0] = t_out.encode(d[0]);
            d[1] = t_out.encode(d[1]);
            d[2] = t_out.encode(d[2]);
        }

        // 8. 3D LUT on encoded values.
        if let Some(stage) = &self.lut {
            apply_lut_slice(&stage.lut, stage.interp, dst);
        }

        // 9. alpha passthrough.
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[3] = s[3];
        }
        Ok(())
    }

    // ── Baking ───────────────────────────────────────────────────────────────

    /// Samples the full pipeline (including its own LUT stage) into a 3D LUT
    /// of the given size — the encoded-in → encoded-out map.
    ///
    /// Loading the baked LUT into a fresh identity pipeline reproduces this
    /// pipeline (up to lattice interpolation error).
    ///
    /// # Errors
    /// [`ColorError::LutSize`] if `size` is outside `2..=129`;
    /// [`ColorError::LutNonFinite`] if the pipeline produces non-finite
    /// output (possible only with a `linear` output transfer on extreme
    /// settings).
    pub fn bake_lut(&self, size: usize) -> Result<Lut3d, ColorError> {
        if !(MIN_LUT_SIZE..=MAX_LUT_SIZE).contains(&size) {
            return Err(ColorError::LutSize { size });
        }
        let mut lut = Lut3d::from_fn(size, |r, g, b| self.sample_encoded(r, g, b))?;
        lut.set_title(Some("OxiMedia ColorPipeline".to_string()));
        Ok(lut)
    }
}

// ── Stage sweeps (branch-hoisted tile kernels) ───────────────────────────────

/// Ops 2–6 over an interleaved RGBA `f32` slice. Alpha lanes carry scratch
/// values (the callers overwrite alpha from the source at the end). Every
/// stage test happens once per tile, not once per pixel, and each enabled
/// stage runs as a tight sweep that LLVM can auto-vectorise.
fn run_linear_stages(
    buf: &mut [f32],
    gain: f32,
    contrast: f32,
    saturation: f32,
    tone_map: Option<&ToneMap>,
    gamut: Option<&GamutMap>,
) {
    // 2. exposure — full-width multiply (alpha lane is scratch).
    if gain != 1.0 {
        for v in buf.iter_mut() {
            *v *= gain;
        }
    }

    // 3. contrast.
    if contrast != 1.0 {
        for px in buf.chunks_exact_mut(4) {
            px[0] = apply_contrast(px[0], contrast);
            px[1] = apply_contrast(px[1], contrast);
            px[2] = apply_contrast(px[2], contrast);
        }
    }

    // 4. saturation (BT.709 luma blend).
    if saturation != 1.0 {
        for px in buf.chunks_exact_mut(4) {
            let luma = B709_R * px[0] + B709_G * px[1] + B709_B * px[2];
            px[0] = luma + (px[0] - luma) * saturation;
            px[1] = luma + (px[1] - luma) * saturation;
            px[2] = luma + (px[2] - luma) * saturation;
        }
    }

    // 5. tone map (operator dispatch hoisted inside).
    if let Some(tm) = tone_map {
        tm.map_slice_rgba(buf);
    }

    // 6. gamut (soft-clip dispatch hoisted inside).
    if let Some(gm) = gamut {
        gm.convert_slice_rgba(buf);
    }
}

/// 3D-LUT sweep over an interleaved RGBA `f32` slice (alpha untouched),
/// with the interpolation-kernel dispatch hoisted out of the loop.
fn apply_lut_slice(lut: &Lut3d, interp: LutInterp, buf: &mut [f32]) {
    match interp {
        LutInterp::Trilinear => {
            for px in buf.chunks_exact_mut(4) {
                let out = lut.sample_trilinear(px[0], px[1], px[2]);
                px[0] = out[0];
                px[1] = out[1];
                px[2] = out[2];
            }
        }
        LutInterp::Tetrahedral => {
            for px in buf.chunks_exact_mut(4) {
                let out = lut.sample_tetrahedral(px[0], px[1], px[2]);
                px[0] = out[0];
                px[1] = out[1];
                px[2] = out[2];
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cube::{export_cube, parse_cube};
    use oximedia_web_core::normalize::f32_to_u8;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn linear_pipeline() -> ColorPipeline {
        let mut p = ColorPipeline::new();
        p.set_input_transfer(Transfer::Linear);
        p.set_output_transfer(Transfer::Linear);
        p
    }

    #[test]
    fn identity_pipeline_u8_is_near_identity() {
        let mut p = ColorPipeline::new();
        let src: Vec<u8> = (0..=255u32).flat_map(|i| [i as u8, i as u8, i as u8, 200]).collect();
        let mut dst = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut dst).expect("apply");
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact(4)) {
            for k in 0..3 {
                assert!(
                    (i32::from(s[k]) - i32::from(d[k])).abs() <= 1,
                    "identity drift at code {}: {} -> {}",
                    s[k],
                    s[k],
                    d[k]
                );
            }
            assert_eq!(s[3], d[3], "alpha must pass through");
        }
    }

    #[test]
    fn exposure_plus_one_stop_doubles_linear_values() {
        let mut p = linear_pipeline();
        p.set_exposure(1.0).expect("exposure");
        let src = [0.1f32, 0.25, 0.4, 1.0, 2.0, 0.5, 0.125, 0.75];
        let mut dst = [0.0f32; 8];
        p.apply_rgba_f32(&src, &mut dst).expect("apply");
        for k in [0usize, 1, 2, 4, 5, 6] {
            assert!(
                approx(dst[k], src[k] * 2.0, 1e-6),
                "channel {k}: {} != 2×{}",
                dst[k],
                src[k]
            );
        }
        // Alpha untouched:
        assert!(approx(dst[3], 1.0, 0.0) && approx(dst[7], 0.75, 0.0));
    }

    #[test]
    fn saturation_zero_yields_bt709_luma_gray() {
        let mut p = linear_pipeline();
        p.set_saturation(0.0).expect("saturation");
        let (r, g, b) = (0.8f32, 0.3f32, 0.1f32);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let src = [r, g, b, 1.0];
        let mut dst = [0.0f32; 4];
        p.apply_rgba_f32(&src, &mut dst).expect("apply");
        for (k, &v) in dst.iter().take(3).enumerate() {
            assert!(approx(v, luma, 1e-6), "channel {k}: {v} != luma {luma}");
        }
    }

    #[test]
    fn contrast_preserves_pivot_and_is_monotonic() {
        let mut p = linear_pipeline();
        p.set_contrast(1.5).expect("contrast");
        let src = [0.18f32, 0.18, 0.18, 1.0];
        let mut dst = [0.0f32; 4];
        p.apply_rgba_f32(&src, &mut dst).expect("apply");
        assert!(approx(dst[0], 0.18, 1e-6), "pivot must be fixed: {}", dst[0]);

        let mut prev = -1.0f32;
        for i in 0..=100 {
            let v = i as f32 / 50.0;
            let mut out = [0.0f32; 4];
            p.apply_rgba_f32(&[v, v, v, 1.0], &mut out).expect("apply");
            assert!(out[0] >= prev - 1e-6, "contrast not monotonic at {v}");
            prev = out[0];
        }
    }

    #[test]
    fn full_pipeline_u8_is_deterministic() {
        let mut p = ColorPipeline::new();
        p.set_exposure(0.7).expect("exposure");
        p.set_contrast(1.1).expect("contrast");
        p.set_saturation(1.2).expect("saturation");
        p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0).expect("tm");
        p.set_gamut(Primaries::Bt2020, Primaries::Bt709).expect("gamut");
        p.set_lut(Lut3d::identity(17).expect("lut"), LutInterp::Tetrahedral);

        let src: Vec<u8> = (0..4096u32)
            .flat_map(|i| {
                let x = (i.wrapping_mul(2654435761)) as u8;
                [x, x.wrapping_add(37), x.wrapping_mul(3), 255]
            })
            .collect();
        let mut a = vec![0u8; src.len()];
        let mut b = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut a).expect("first");
        p.apply_rgba8(&src, &mut b).expect("second");
        assert_eq!(a, b, "same input must produce identical output");
    }

    #[test]
    fn u8_path_matches_exact_path_closely() {
        let mut p = ColorPipeline::new();
        p.set_exact(true); // this test pins the exact tile path specifically
        p.set_exposure(0.5).expect("exposure");
        p.set_tone_map(ToneMapOperator::Hable, 1000.0, 100.0).expect("tm");
        let src: Vec<u8> = (0..=255u32).flat_map(|i| [i as u8, 128, 64, 255]).collect();
        let mut dst = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut dst).expect("apply");
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact(4)) {
            let exact = p.sample_encoded(
                u8_to_f32(s[0]),
                u8_to_f32(s[1]),
                u8_to_f32(s[2]),
            );
            for k in 0..3 {
                let want = f32_to_u8(exact[k]);
                assert!(
                    (i32::from(d[k]) - i32::from(want)).abs() <= 1,
                    "u8/exact drift at ({},{},{}) ch{k}: {} vs {}",
                    s[0],
                    s[1],
                    s[2],
                    d[k],
                    want
                );
            }
        }
    }

    /// Deterministic sweep image covering the RGB cube (plus alpha variety).
    fn sweep_image() -> Vec<u8> {
        let mut img = Vec::with_capacity(18 * 18 * 18 * 4);
        for r in (0..=255u32).step_by(15) {
            for g in (0..=255u32).step_by(15) {
                for b in (0..=255u32).step_by(15) {
                    img.extend_from_slice(&[r as u8, g as u8, b as u8, (r ^ g ^ b) as u8]);
                }
            }
        }
        img
    }

    /// Runs the sweep through `p` in baked then exact mode, returning
    /// `(max_delta, mean_delta, fraction_over_two)` across RGB channels.
    #[allow(clippy::cast_precision_loss)]
    fn baked_vs_exact_stats(p: &mut ColorPipeline) -> (i32, f64, f64) {
        let src = sweep_image();
        let mut baked = vec![0u8; src.len()];
        p.set_exact(false);
        p.apply_rgba8(&src, &mut baked).expect("baked apply");
        let mut exact = vec![0u8; src.len()];
        p.set_exact(true);
        p.apply_rgba8(&src, &mut exact).expect("exact apply");

        let mut max_delta = 0i32;
        let mut sum = 0u64;
        let mut over_two = 0u64;
        let mut n = 0u64;
        for (b_px, e_px) in baked.chunks_exact(4).zip(exact.chunks_exact(4)) {
            for k in 0..3 {
                let d = (i32::from(b_px[k]) - i32::from(e_px[k])).abs();
                max_delta = max_delta.max(d);
                sum += d as u64;
                over_two += u64::from(d > 2);
                n += 1;
            }
            assert_eq!(b_px[3], e_px[3], "alpha must pass through in both modes");
        }
        (max_delta, sum as f64 / n as f64, over_two as f64 / n as f64)
    }

    #[test]
    fn baked_matches_exact_within_two_codes_for_smooth_chains() {
        // Documented tolerance of the default baked u8 path for smooth
        // operator chains (transfers, exposure, contrast, tone map, smooth
        // LUTs): max channel delta <= 2/255 vs the exact chain across a
        // full-cube sweep.
        let mut p = ColorPipeline::new();
        assert!(!p.is_exact(), "baked mode must be the default");
        p.set_exposure(0.5).expect("exposure");
        p.set_contrast(1.2).expect("contrast");
        p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0).expect("tm");
        p.set_lut(Lut3d::identity(33).expect("lut"), LutInterp::Tetrahedral);
        let (max_delta, _, _) = baked_vs_exact_stats(&mut p);
        assert!(
            max_delta <= 2,
            "smooth-chain baked path drifted {max_delta} codes (documented tolerance: 2)"
        );

        // HDR-style transfer conversion is smooth too.
        let mut p = ColorPipeline::new();
        p.set_input_transfer(Transfer::Hlg);
        p.set_output_transfer(Transfer::Srgb);
        p.set_tone_map(ToneMapOperator::Reinhard, 1000.0, 100.0).expect("tm");
        let (max_delta, _, _) = baked_vs_exact_stats(&mut p);
        assert!(
            max_delta <= 2,
            "HLG->sRGB baked path drifted {max_delta} codes (documented tolerance: 2)"
        );
    }

    #[test]
    fn baked_clip_kinks_deviate_only_locally() {
        // Chains with hard clip surfaces (gamut fix-up, saturation-induced
        // negative-channel clipping) are not piecewise-trilinear: a 33³
        // lattice cannot track the kink exactly, so error concentrates in
        // the cells the clip surface crosses. Documented behaviour: the
        // overwhelming majority of the cube stays within the smooth-chain
        // tolerance, and colorimetric-critical callers use set_exact(true).
        // (This torture chain — BT.2020→709 clip + saturation 1.3 + a
        // sqrt-shaped LUT — measures max 45, mean 0.16, 1.2% over 2 codes.)
        let mut p = ColorPipeline::new();
        p.set_exposure(0.5).expect("exposure");
        p.set_contrast(1.2).expect("contrast");
        p.set_saturation(1.3).expect("saturation");
        p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0).expect("tm");
        p.set_gamut(Primaries::Bt2020, Primaries::Bt709).expect("gamut");
        p.set_lut(
            Lut3d::from_fn(17, |r, g, b| [r.sqrt(), g, b * b]).expect("lut"),
            LutInterp::Tetrahedral,
        );
        let (max_delta, mean, frac_over_two) = baked_vs_exact_stats(&mut p);
        assert!(max_delta <= 64, "kink deviation blew past its bound: {max_delta}");
        assert!(mean <= 0.5, "mean deviation must stay sub-code: {mean}");
        assert!(
            frac_over_two <= 0.05,
            "deviation must be localised to clip surfaces: {:.2}% of channels over 2 codes",
            frac_over_two * 100.0
        );
    }

    #[test]
    fn dirty_flag_rebakes_on_config_change() {
        let src = sweep_image();

        let mut p = ColorPipeline::new();
        p.set_exposure(0.0).expect("exposure");
        let mut before = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut before).expect("first apply (bakes)");

        // Reconfigure after the bake: the next apply must reflect it.
        p.set_exposure(1.5).expect("exposure change");
        let mut after = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut after).expect("second apply (rebakes)");
        assert_ne!(before, after, "config change after a bake must change the output");

        // And the rebaked output must equal a fresh pipeline with the same
        // final configuration (i.e. the rebake was complete, not partial).
        let mut fresh = ColorPipeline::new();
        fresh.set_exposure(1.5).expect("exposure");
        let mut fresh_out = vec![0u8; src.len()];
        fresh.apply_rgba8(&src, &mut fresh_out).expect("fresh apply");
        assert_eq!(after, fresh_out, "rebaked output must match a fresh identical pipeline");
    }

    #[test]
    fn baked_identity_pipeline_is_identity() {
        let mut p = ColorPipeline::new(); // sRGB in, sRGB out, neutral ops
        let src = sweep_image();
        let mut dst = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut dst).expect("apply");
        assert_eq!(src, dst, "baked identity pipeline must be byte-identical");
    }

    #[test]
    fn in_place_matches_two_buffer_path() {
        let mut p = ColorPipeline::new();
        p.set_exposure(0.7).expect("exposure");
        p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0).expect("tm");
        p.set_lut(Lut3d::identity(9).expect("lut"), LutInterp::Tetrahedral);

        let src: Vec<u8> = (0..9000u32).map(|i| (i.wrapping_mul(97)) as u8).collect();
        let mut two_buf = vec![0u8; src.len()];
        p.apply_rgba8(&src, &mut two_buf).expect("two-buffer");
        let mut in_place = src.clone();
        p.apply_rgba8_in_place(&mut in_place).expect("in-place");
        assert_eq!(two_buf, in_place, "in-place must equal the two-buffer path");

        assert!(matches!(
            p.apply_rgba8_in_place(&mut [0u8; 6]),
            Err(ColorError::NotRgba { .. })
        ));
    }

    #[test]
    fn buffer_validation_errors() {
        let mut p = ColorPipeline::new();
        let src = [0u8; 8];
        let mut small = [0u8; 4];
        assert!(matches!(
            p.apply_rgba8(&src, &mut small),
            Err(ColorError::LengthMismatch { .. })
        ));
        let src3 = [0u8; 6];
        let mut dst3 = [0u8; 6];
        assert!(matches!(
            p.apply_rgba8(&src3, &mut dst3),
            Err(ColorError::NotRgba { .. })
        ));
        let fsrc = [0.0f32; 6];
        let mut fdst = [0.0f32; 6];
        assert!(p.apply_rgba_f32(&fsrc, &mut fdst).is_err());
    }

    #[test]
    fn setter_validation() {
        let mut p = ColorPipeline::new();
        assert!(p.set_exposure(f32::NAN).is_err());
        assert!(p.set_exposure(40.0).is_err());
        assert!(p.set_contrast(0.0).is_err());
        assert!(p.set_contrast(-1.0).is_err());
        assert!(p.set_saturation(-0.1).is_err());
        assert!(p.set_saturation(f32::INFINITY).is_err());
        assert!(p.set_gamut_softness(0.5).is_err(), "no gamut stage yet");
        p.set_gamut(Primaries::Bt2020, Primaries::Bt709).expect("gamut");
        assert!(p.set_gamut_softness(0.5).is_ok());
    }

    #[test]
    fn hlg_to_srgb_conversion_is_sane() {
        let mut p = ColorPipeline::new();
        p.set_input_transfer(Transfer::Hlg);
        p.set_output_transfer(Transfer::Srgb);
        p.set_tone_map(ToneMapOperator::Reinhard, 1000.0, 100.0).expect("tm");
        let src = [128u8, 128, 128, 255, 255, 255, 255, 255, 0, 0, 0, 255];
        let mut dst = [0u8; 12];
        p.apply_rgba8(&src, &mut dst).expect("apply");
        // Black stays black, white stays bright, mid stays between.
        assert!(dst[8] == 0 && dst[9] == 0 && dst[10] == 0);
        assert!(dst[4] > dst[0], "white must be brighter than mid gray");
    }

    #[test]
    fn bake_identity_pipeline_yields_identity_lut() {
        let mut p = linear_pipeline();
        let lut = p.bake_lut(9).expect("bake");
        for i in 0..=16 {
            let v = i as f32 / 16.0;
            let out = lut.sample(LutInterp::Tetrahedral, v, v, v);
            for (k, &o) in out.iter().enumerate() {
                assert!(approx(o, v, 1e-4), "identity bake at {v} ch{k}: {o}");
            }
        }
        // keep p "used" as &mut for parity with the wasm layer
        let _ = p.apply_rgba_f32(&[0.0; 4], &mut [0.0; 4]);
    }

    #[test]
    fn bake_export_parse_reproduces_pipeline() {
        let mut p = ColorPipeline::new();
        p.set_exposure(0.4).expect("exposure");
        p.set_saturation(0.8).expect("saturation");
        p.set_tone_map(ToneMapOperator::Aces, 600.0, 100.0).expect("tm");

        let baked = p.bake_lut(33).expect("bake");
        let text = export_cube(&baked);
        let parsed = parse_cube(&text).expect("parse");
        assert_eq!(parsed.title(), Some("OxiMedia ColorPipeline"));

        // A fresh pipeline with only the parsed LUT must match the original.
        let mut q = ColorPipeline::new();
        q.set_input_transfer(Transfer::Linear); // LUT operates on encoded values;
        q.set_output_transfer(Transfer::Linear); // feed them straight through.
        q.set_lut(parsed, LutInterp::Tetrahedral);

        for i in 0..=20 {
            let x = i as f32 / 20.0;
            let want = p.sample_encoded(x, x * 0.5, 1.0 - x);
            let got = q.sample_encoded(x, x * 0.5, 1.0 - x);
            for k in 0..3 {
                assert!(
                    approx(want[k], got[k], 5e-3),
                    "bake round-trip at {x} ch{k}: {} vs {}",
                    want[k],
                    got[k]
                );
            }
        }
    }

    #[test]
    fn bake_size_bounds() {
        let p = ColorPipeline::new();
        assert!(matches!(p.bake_lut(1), Err(ColorError::LutSize { .. })));
        assert!(matches!(p.bake_lut(200), Err(ColorError::LutSize { .. })));
        assert!(p.bake_lut(2).is_ok());
    }

    #[test]
    fn lut_stage_applies_on_encoded_values() {
        // A LUT that swaps R and B: verify it runs after encoding.
        let swap = Lut3d::from_fn(5, |r, g, b| [b, g, r]).expect("swap lut");
        let mut p = ColorPipeline::new();
        p.set_lut(swap, LutInterp::Trilinear);
        let src = [200u8, 100, 50, 255];
        let mut dst = [0u8; 4];
        p.apply_rgba8(&src, &mut dst).expect("apply");
        assert!(
            (i32::from(dst[0]) - 50).abs() <= 1 && (i32::from(dst[2]) - 200).abs() <= 1,
            "swap LUT must exchange R/B: {dst:?}"
        );
    }

    #[test]
    fn pq_output_deep_blacks_are_accurate_on_u8_path() {
        // Regression guard for the sqrt-domain encode LUT: PQ's steep toe.
        let mut p = ColorPipeline::new();
        p.set_input_transfer(Transfer::Linear);
        p.set_output_transfer(Transfer::Pq);
        for lin in [0.0f32, 1e-5, 1e-4, 1e-3, 0.01, 0.1, 0.5, 1.0] {
            let src = [lin, lin, lin, 1.0];
            let mut dst = [0.0f32; 4];
            p.apply_rgba_f32(&src, &mut dst).expect("apply");
            let exact = Transfer::Pq.encode(lin);
            assert!(
                approx(dst[0], exact, 1e-6),
                "f32 PQ encode at {lin}: {} vs {exact}",
                dst[0]
            );
        }
    }
}
