//! Per-frame symbol counters for VP9 backward probability adaptation —
//! an exact mirror of libvpx `FRAME_COUNTS`
//! (v1.15.2 `vp9/common/vp9_entropymode.h:62-79`).
//!
//! A non-error-resilient, non-frame-parallel VP9 frame ends with a *backward*
//! adaptation pass (`vp9_adapt_coef_probs` / `vp9_adapt_mode_probs` /
//! `vp9_adapt_mv_probs`, `vp9_decodeframe.c:3049-3057`) that folds the symbol
//! counts observed while decoding the frame back into the working entropy
//! context before it is saved to `frame_contexts[frame_context_idx]`. The
//! counters those functions consume live here; [`super::adapt`] consumes
//! them.
//!
//! # Which of these are filled in today
//!
//! Only the intra decode path exists in this crate
//! ([`super::recon`]), so only the counters that path produces are written:
//! [`FrameCounts::coef`] / [`FrameCounts::eob_branch`] (`vp9_detokenize.c`
//! `decode_coefs`), [`FrameCounts::skip`] (`vp9_decodemv.c:187`),
//! [`FrameCounts::tx`] (`vp9_decodemv.c:76`) and
//! [`FrameCounts::partition`] (`vp9_decodeframe.c:1167`) — exactly the set
//! libvpx's `read_intra_frame_mode_info` / `read_partition` / `decode_coefs`
//! increment on an intra frame. The inter-only counters
//! ([`FrameCounts::y_mode`], [`FrameCounts::uv_mode`],
//! [`FrameCounts::inter_mode`], [`FrameCounts::intra_inter`],
//! [`FrameCounts::comp_inter`], [`FrameCounts::single_ref`],
//! [`FrameCounts::comp_ref`], [`FrameCounts::switchable_interp`],
//! [`FrameCounts::mv`]) stay zero because the syntax that produces them is
//! not decoded yet — the full shape is declared here so the inter package
//! fills fields rather than reshaping the struct, and so
//! [`super::adapt`]'s ports can be written and unit-tested against the real
//! layout.
//!
//! Of the counters that *are* filled, only `coef`/`eob_branch` currently
//! change any decode: libvpx runs `vp9_adapt_mode_probs` (skip / tx /
//! partition) **only** for `!frame_is_intra_only` frames
//! (`vp9_decodeframe.c:3053`), so on an intra frame the skip/tx/partition
//! counts are accumulated exactly as libvpx accumulates them and then, also
//! exactly as libvpx does, go unused.

#![forbid(unsafe_code)]

use super::tables_inter::{
    CLASS0_SIZE, COMP_INTER_CONTEXTS, INTER_MODES, INTER_MODE_CONTEXTS, INTRA_INTER_CONTEXTS,
    MV_CLASSES, MV_FP_SIZE, MV_JOINTS, MV_OFFSET_BITS, REF_CONTEXTS, SWITCHABLE_FILTERS,
    SWITCHABLE_FILTER_CONTEXTS,
};

/// Number of transform sizes (libvpx `vp9_enums.h`: `TX_SIZES 4`).
pub const TX_SIZES: usize = 4;
/// Luma / chroma coefficient plane types (libvpx `vp9_entropy.h`).
pub const PLANE_TYPES: usize = 2;
/// Intra / inter coefficient reference types (`vp9_entropy.h:86`).
pub const REF_TYPES: usize = 2;
/// Coefficient bands (`vp9_entropy.h:89`).
pub const COEF_BANDS: usize = 6;
/// Coefficient contexts (`vp9_entropy.h:107`).
pub const COEFF_CONTEXTS: usize = 6;
/// Coded coefficient tree nodes per (band, ctx) (`vp9_entropy.h:142`).
pub const UNCONSTRAINED_NODES: usize = 3;
/// Counter slots per (band, ctx): the three coded nodes plus the EOB model
/// token (`vp9_coeff_count_model`, `vp9_entropy.h:153-155`).
pub const COEF_COUNT_TOKENS: usize = UNCONSTRAINED_NODES + 1;
/// Skip-flag contexts (libvpx `vp9_blockd.h` `SKIP_CONTEXTS`).
pub const SKIP_CONTEXTS: usize = 3;
/// tx-size probability contexts (`vp9_entropymode.h:25`).
pub const TX_SIZE_CONTEXTS: usize = 2;
/// Block-size groups for the y-mode probabilities (`vp9_entropymode.h:23`).
pub const BLOCK_SIZE_GROUPS: usize = 4;
/// Intra prediction modes (libvpx `INTRA_MODES`, DC_PRED..=TM_PRED).
pub const INTRA_MODES: usize = 10;
/// Partition contexts (`vp9_enums.h:74`: `4 * PARTITION_PLOFFSET`).
pub const PARTITION_CONTEXTS: usize = 16;
/// Partition types (`vp9_enums.h:68`).
pub const PARTITION_TYPES: usize = 4;

/// `ZERO_TOKEN` counter slot (`vp9_entropy.h:27`).
pub const ZERO_TOKEN: usize = 0;
/// `ONE_TOKEN` counter slot (`vp9_entropy.h:28`).
pub const ONE_TOKEN: usize = 1;
/// `TWO_TOKEN` counter slot (`vp9_entropy.h:29`).
pub const TWO_TOKEN: usize = 2;
/// `EOB_MODEL_TOKEN` counter slot (`vp9_entropy.h:76`).
pub const EOB_MODEL_TOKEN: usize = 3;

/// `vp9_coeff_count_model[TX_SIZES]`, i.e. libvpx's
/// `counts.coef[tx][plane][ref][band][ctx][token]`.
pub type CoefCounts = [[[[[[u32; COEF_COUNT_TOKENS]; COEFF_CONTEXTS]; COEF_BANDS]; REF_TYPES];
    PLANE_TYPES]; TX_SIZES];

/// `counts.eob_branch[tx][plane][ref][band][ctx]`: how many times the EOB
/// branch at that node was *read* (the denominator `neob` is measured
/// against).
pub type EobBranchCounts =
    [[[[[u32; COEFF_CONTEXTS]; COEF_BANDS]; REF_TYPES]; PLANE_TYPES]; TX_SIZES];

/// libvpx `struct tx_counts` (`vp9_entropymode.h:37-42`).
///
/// The array a block increments is chosen by its *maximum* transform size
/// (`get_tx_counts`, `vp9_pred_common.h:183-191`), and the index within it
/// is the transform size actually coded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxCounts {
    /// Blocks whose max tx size is 32x32: counts per coded tx size.
    pub p32x32: [[u32; TX_SIZES]; TX_SIZE_CONTEXTS],
    /// Blocks whose max tx size is 16x16.
    pub p16x16: [[u32; TX_SIZES - 1]; TX_SIZE_CONTEXTS],
    /// Blocks whose max tx size is 8x8.
    pub p8x8: [[u32; TX_SIZES - 2]; TX_SIZE_CONTEXTS],
    /// Per-tx-size totals (`tx_totals`); libvpx's decoder never writes these
    /// — only its encoder does — so they stay zero here, faithfully.
    pub tx_totals: [u32; TX_SIZES],
}

/// libvpx `nmv_component_counts` (`vp9_entropymv.h:114-123`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NmvComponentCounts {
    /// Sign bit counts.
    pub sign: [u32; 2],
    /// Magnitude-class counts.
    pub classes: [u32; MV_CLASSES],
    /// Class-0 magnitude counts.
    pub class0: [u32; CLASS0_SIZE],
    /// Integer-magnitude bit counts, `[bit][value]`.
    pub bits: [[u32; 2]; MV_OFFSET_BITS],
    /// Class-0 fractional-pel counts.
    pub class0_fp: [[u32; MV_FP_SIZE]; CLASS0_SIZE],
    /// Fractional-pel counts for classes above 0.
    pub fp: [u32; MV_FP_SIZE],
    /// Class-0 high-precision bit counts.
    pub class0_hp: [u32; 2],
    /// High-precision bit counts for classes above 0.
    pub hp: [u32; 2],
}

/// libvpx `nmv_context_counts` (`vp9_entropymv.h:125-128`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NmvCounts {
    /// Motion-vector joint counts.
    pub joints: [u32; MV_JOINTS],
    /// Per-component (row, col) counters.
    pub comps: [NmvComponentCounts; 2],
}

/// Symbol counts accumulated while decoding one frame (libvpx
/// `FRAME_COUNTS`, `vp9_entropymode.h:62-79`), field for field and in the
/// same order.
///
/// `Debug` is implemented by hand (see below) rather than derived: a derived
/// one dumps ~3300 counters. `PartialEq` is derived so tests can assert a
/// counter set is exactly zero, or exactly unchanged.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FrameCounts {
    /// `y_mode[block_size_group][mode]`.
    pub y_mode: [[u32; INTRA_MODES]; BLOCK_SIZE_GROUPS],
    /// `uv_mode[y_mode][uv_mode]`.
    pub uv_mode: [[u32; INTRA_MODES]; INTRA_MODES],
    /// `partition[ctx][partition_type]`.
    pub partition: [[u32; PARTITION_TYPES]; PARTITION_CONTEXTS],
    /// `coef[tx][plane][ref][band][ctx][token]`.
    pub coef: CoefCounts,
    /// `eob_branch[tx][plane][ref][band][ctx]`.
    pub eob_branch: EobBranchCounts,
    /// `switchable_interp[ctx][filter]`.
    pub switchable_interp: [[u32; SWITCHABLE_FILTERS]; SWITCHABLE_FILTER_CONTEXTS],
    /// `inter_mode[ctx][mode]`.
    pub inter_mode: [[u32; INTER_MODES]; INTER_MODE_CONTEXTS],
    /// `intra_inter[ctx][is_inter]`.
    pub intra_inter: [[u32; 2]; INTRA_INTER_CONTEXTS],
    /// `comp_inter[ctx][is_compound]`.
    pub comp_inter: [[u32; 2]; COMP_INTER_CONTEXTS],
    /// `single_ref[ctx][bit][value]`.
    pub single_ref: [[[u32; 2]; 2]; REF_CONTEXTS],
    /// `comp_ref[ctx][value]`.
    pub comp_ref: [[u32; 2]; REF_CONTEXTS],
    /// Transform-size counts.
    pub tx: TxCounts,
    /// `skip[ctx][skip]`.
    pub skip: [[u32; 2]; SKIP_CONTEXTS],
    /// Motion-vector counts.
    pub mv: NmvCounts,
}

impl FrameCounts {
    /// Creates an all-zero counter set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears every counter (libvpx `vp9_zero(cm->counts)`,
    /// `vp9_decodeframe.c:2787`), called at the start of each frame that will
    /// run backward adaptation.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Total number of coefficient tokens counted, over every transform size,
    /// plane, reference type, band and context.
    ///
    /// Only a summary for tests and diagnostics — adaptation reads the
    /// individual counters, never this.
    #[must_use]
    pub fn total_coef_tokens(&self) -> u64 {
        let mut total = 0u64;
        for tx in &self.coef {
            for plane in tx {
                for r in plane {
                    for band in r {
                        for ctx in band {
                            for &n in ctx {
                                total += u64::from(n);
                            }
                        }
                    }
                }
            }
        }
        total
    }

    /// Total number of EOB-branch reads counted.
    #[must_use]
    pub fn total_eob_branches(&self) -> u64 {
        let mut total = 0u64;
        for tx in &self.eob_branch {
            for plane in tx {
                for r in plane {
                    for band in r {
                        for &n in band {
                            total += u64::from(n);
                        }
                    }
                }
            }
        }
        total
    }

    /// True when every counter is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

impl std::fmt::Debug for FrameCounts {
    /// Summarises rather than dumping every counter.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let skip: u32 = self.skip.iter().flatten().sum();
        let partition: u32 = self.partition.iter().flatten().sum();
        f.debug_struct("FrameCounts")
            .field("coef_tokens", &self.total_coef_tokens())
            .field("eob_branches", &self.total_eob_branches())
            .field("skip", &skip)
            .field("partition", &partition)
            .field("all_zero", &self.is_zero())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_zeroed() {
        let counts = FrameCounts::new();
        assert!(counts.is_zero());
        assert_eq!(counts.total_coef_tokens(), 0);
        assert_eq!(counts.total_eob_branches(), 0);
    }

    #[test]
    fn reset_clears_counters() {
        let mut counts = FrameCounts::new();
        counts.coef[3][1][1][5][5][EOB_MODEL_TOKEN] = 7;
        counts.eob_branch[0][0][0][0][0] = 9;
        counts.skip[2][1] = 4;
        counts.partition[15][3] = 2;
        counts.tx.p32x32[1][3] = 5;
        counts.mv.comps[1].class0_hp[0] = 3;
        assert!(!counts.is_zero());
        counts.reset();
        assert!(counts.is_zero());
        assert!(counts == FrameCounts::default());
    }

    /// The struct is a mirror of libvpx `FRAME_COUNTS`; the dimensions are
    /// what the adaptation ports index with, so they are asserted rather
    /// than assumed.
    #[test]
    fn shapes_match_libvpx_frame_counts() {
        let c = FrameCounts::new();
        assert_eq!(c.y_mode.len(), 4);
        assert_eq!(c.y_mode[0].len(), 10);
        assert_eq!(c.uv_mode.len(), 10);
        assert_eq!(c.uv_mode[0].len(), 10);
        assert_eq!(c.partition.len(), 16);
        assert_eq!(c.partition[0].len(), 4);
        assert_eq!(c.coef.len(), 4);
        assert_eq!(c.coef[0].len(), 2);
        assert_eq!(c.coef[0][0].len(), 2);
        assert_eq!(c.coef[0][0][0].len(), 6);
        assert_eq!(c.coef[0][0][0][0].len(), 6);
        assert_eq!(
            c.coef[0][0][0][0][0].len(),
            4,
            "UNCONSTRAINED_NODES + 1 (three coded nodes + EOB model token)"
        );
        assert_eq!(c.eob_branch[0][0][0][0].len(), 6);
        assert_eq!(c.switchable_interp.len(), 4);
        assert_eq!(c.switchable_interp[0].len(), 3);
        assert_eq!(c.inter_mode.len(), 7);
        assert_eq!(c.inter_mode[0].len(), 4);
        assert_eq!(c.intra_inter.len(), 4);
        assert_eq!(c.comp_inter.len(), 5);
        assert_eq!(c.single_ref.len(), 5);
        assert_eq!(c.single_ref[0].len(), 2);
        assert_eq!(c.comp_ref.len(), 5);
        assert_eq!(c.tx.p32x32[0].len(), 4);
        assert_eq!(c.tx.p16x16[0].len(), 3);
        assert_eq!(c.tx.p8x8[0].len(), 2);
        assert_eq!(c.skip.len(), 3);
        assert_eq!(c.mv.joints.len(), 4);
        assert_eq!(c.mv.comps[0].classes.len(), 11);
        assert_eq!(c.mv.comps[0].class0.len(), 2);
        assert_eq!(c.mv.comps[0].bits.len(), 10);
        assert_eq!(c.mv.comps[0].class0_fp.len(), 2);
        assert_eq!(c.mv.comps[0].class0_fp[0].len(), 4);
        assert_eq!(c.mv.comps[0].fp.len(), 4);
    }

    #[test]
    fn token_slots_match_libvpx_indices() {
        assert_eq!((ZERO_TOKEN, ONE_TOKEN, TWO_TOKEN), (0, 1, 2));
        assert_eq!(EOB_MODEL_TOKEN, 3, "vp9_entropy.h:76");
    }

    #[test]
    fn totals_sum_every_dimension() {
        let mut counts = FrameCounts::new();
        counts.coef[0][0][0][0][0][ZERO_TOKEN] = 1;
        counts.coef[3][1][1][5][5][EOB_MODEL_TOKEN] = 2;
        counts.eob_branch[1][0][1][2][3] = 5;
        counts.eob_branch[3][1][0][5][5] = 6;
        assert_eq!(counts.total_coef_tokens(), 3);
        assert_eq!(counts.total_eob_branches(), 11);
    }

    /// The compact `Debug` must stay useful: it has to report the totals and
    /// the all-zero flag rather than a wall of zeros.
    #[test]
    fn debug_is_a_summary() {
        let mut counts = FrameCounts::new();
        counts.coef[0][0][0][0][0][ONE_TOKEN] = 3;
        let text = format!("{counts:?}");
        assert!(text.contains("coef_tokens: 3"), "{text}");
        assert!(text.contains("all_zero: false"), "{text}");
        assert!(
            text.len() < 400,
            "Debug must summarise, got {} bytes",
            text.len()
        );
    }
}
