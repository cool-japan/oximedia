//! Four-lane 32-bit-integer SIMD abstraction shared by the AV1 kernels.
//!
//! The AV1 decode kernels ([`crate::av1_loopfilter`]) are written *once*
//! against the [`Simd4`] trait and monomorphised onto every backend the
//! target provides.  This is a
//! deliberate design choice for bit-exactness: there is exactly one copy of
//! each algorithm, so a backend cannot silently drift from the reference.
//!
//! Every operation in the trait is an elementwise integer operation with
//! identical semantics on all backends (two's-complement wrapping is never
//! relied upon; all intermediate values in the AV1 kernels fit comfortably in
//! `i32`).  In particular:
//!
//! * [`Simd4::shr`] is an **arithmetic** shift, matching Rust's `>>` on `i32`.
//! * [`Simd4::gt`] produces an all-ones / all-zeros lane mask.
//! * [`Simd4::select`] is a pure bitwise blend, so it never introduces
//!   lane-dependent control flow.
//!
//! # Backends
//!
//! | Backend      | Type                     | Availability                    |
//! |--------------|--------------------------|---------------------------------|
//! | [`Portable`] | `[i32; 4]`               | always (this is the scalar path)|
//! | [`Neon`]     | `core::arch::aarch64`    | `aarch64` + `neon`              |
//!
//! [`Portable`] is a genuine scalar fallback: it contains no intrinsics and
//! is compiled and exercised by the test-suite on every target.

#![allow(dead_code)]

/// Four lanes of `i32`, with exact elementwise integer semantics.
///
/// All methods are `#[inline(always)]` in the implementations so the
/// monomorphised kernels compile down to straight-line SIMD code.
///
/// The trait is the *backend contract*, not a minimal set of what today's
/// one kernel happens to call: `mul`, `load` and `store` currently have no
/// caller, but they are part of what a new backend must get right and every
/// method is checked against plain `i32` arithmetic by the conformance test
/// at the bottom of this file.
pub(crate) trait Simd4: Copy {
    /// Broadcast one value to all four lanes.
    fn splat(v: i32) -> Self;
    /// Load four lanes from an array (lane `i` ← `a[i]`).
    fn from_array(a: [i32; 4]) -> Self;
    /// Store four lanes to an array.
    fn to_array(self) -> [i32; 4];
    /// Loads four lanes directly from the start of `src`.
    ///
    /// Prefer this over [`Simd4::from_array`] when the source is already a
    /// slice: building an array first forces the value through the stack and
    /// defeats the point of the vector load.
    ///
    /// # Panics
    ///
    /// Panics if `src.len() < 4`.
    fn load(src: &[i32]) -> Self;
    /// Stores four lanes directly to the start of `dst`.
    ///
    /// # Panics
    ///
    /// Panics if `dst.len() < 4`.
    fn store(self, dst: &mut [i32]);

    /// Lanewise `a + b`.
    fn add(self, o: Self) -> Self;
    /// Lanewise `a - b`.
    fn sub(self, o: Self) -> Self;
    /// Lanewise `a * b` (low 32 bits).
    fn mul(self, o: Self) -> Self;
    /// Lanewise `min(a, b)`.
    fn min(self, o: Self) -> Self;
    /// Lanewise `max(a, b)`.
    fn max(self, o: Self) -> Self;
    /// Lanewise `|a|`.
    fn abs(self) -> Self;

    /// Lanewise arithmetic right shift by the constant `N`.
    fn shr<const N: i32>(self) -> Self;

    /// Lanewise `a > b`, as an all-ones / all-zeros mask.
    fn gt(self, o: Self) -> Self;
    /// Lanewise bitwise AND.
    fn and(self, o: Self) -> Self;
    /// Lanewise bitwise OR.
    fn or(self, o: Self) -> Self;
    /// Lanewise `(!self) & o`.
    fn andnot(self, o: Self) -> Self;
    /// True when any lane has any bit set.
    fn any(self) -> bool;

    /// Bitwise blend: lanes where `mask` is all-ones take `a`, else `b`.
    #[inline(always)]
    fn select(mask: Self, a: Self, b: Self) -> Self {
        mask.and(a).or(mask.andnot(b))
    }

    /// Lanewise `v.clamp(lo, hi)`.
    #[inline(always)]
    fn clamp(self, lo: Self, hi: Self) -> Self {
        self.max(lo).min(hi)
    }
}

// ── Portable backend (the scalar fallback) ───────────────────────────────────

/// Portable four-lane backend built from plain `i32` arithmetic.
///
/// This is the scalar fallback path.  It is always compiled, is selected on
/// any target without a hand-written backend, and is the oracle the
/// architecture backends are checked against in
/// `crate::av1_loopfilter::tests`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Portable(pub [i32; 4]);

impl Simd4 for Portable {
    #[inline(always)]
    fn splat(v: i32) -> Self {
        Self([v; 4])
    }
    #[inline(always)]
    fn from_array(a: [i32; 4]) -> Self {
        Self(a)
    }
    #[inline(always)]
    fn to_array(self) -> [i32; 4] {
        self.0
    }
    #[inline(always)]
    fn load(src: &[i32]) -> Self {
        Self([src[0], src[1], src[2], src[3]])
    }
    #[inline(always)]
    fn store(self, dst: &mut [i32]) {
        dst[..4].copy_from_slice(&self.0);
    }
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]])
    }
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]])
    }
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]])
    }
    #[inline(always)]
    fn min(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].min(b[2]),
            a[3].min(b[3]),
        ])
    }
    #[inline(always)]
    fn max(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([
            a[0].max(b[0]),
            a[1].max(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ])
    }
    #[inline(always)]
    fn abs(self) -> Self {
        let a = self.0;
        Self([a[0].abs(), a[1].abs(), a[2].abs(), a[3].abs()])
    }
    #[inline(always)]
    fn shr<const N: i32>(self) -> Self {
        let a = self.0;
        Self([a[0] >> N, a[1] >> N, a[2] >> N, a[3] >> N])
    }
    #[inline(always)]
    fn gt(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        let m = |x: bool| if x { -1i32 } else { 0 };
        Self([
            m(a[0] > b[0]),
            m(a[1] > b[1]),
            m(a[2] > b[2]),
            m(a[3] > b[3]),
        ])
    }
    #[inline(always)]
    fn and(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([a[0] & b[0], a[1] & b[1], a[2] & b[2], a[3] & b[3]])
    }
    #[inline(always)]
    fn or(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([a[0] | b[0], a[1] | b[1], a[2] | b[2], a[3] | b[3]])
    }
    #[inline(always)]
    fn andnot(self, o: Self) -> Self {
        let (a, b) = (self.0, o.0);
        Self([!a[0] & b[0], !a[1] & b[1], !a[2] & b[2], !a[3] & b[3]])
    }
    #[inline(always)]
    fn any(self) -> bool {
        let a = self.0;
        (a[0] | a[1] | a[2] | a[3]) != 0
    }
}

/// Whether the NEON backend may be used on the executing CPU.
///
/// The NEON backend is gated on `target_feature = "neon"` being *statically*
/// enabled, which is the aarch64 platform baseline.  Where that gate holds
/// the answer is already a compile-time constant, so this folds to `true` and
/// no CPU query survives optimisation — important because these kernels are
/// called once per prediction row and once per filtered edge, where even a
/// cached `OnceLock` lookup would be significant and would block inlining.
/// The runtime query remains as the fallback for targets where NEON is only
/// dynamically available.
#[inline(always)]
pub(crate) fn neon_available() -> bool {
    cfg!(target_feature = "neon") || crate::detect_cpu_features().neon
}

// ── NEON backend ─────────────────────────────────────────────────────────────

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
pub(crate) use neon_backend::Neon;

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
mod neon_backend {
    use super::Simd4;
    use core::arch::aarch64::{
        int32x4_t, vabsq_s32, vaddq_s32, vandq_s32, vbicq_s32, vcgtq_s32, vdupq_n_s32, vld1q_s32,
        vmaxq_s32, vmaxvq_u32, vminq_s32, vmulq_s32, vorrq_s32, vreinterpretq_s32_u32,
        vreinterpretq_u32_s32, vshrq_n_s32, vst1q_s32, vsubq_s32,
    };

    /// AArch64 NEON four-lane backend.
    ///
    /// NEON is part of the `aarch64` baseline, so this module is gated on
    /// `target_feature = "neon"` being statically enabled — every intrinsic
    /// below is then unconditionally legal on the executing CPU and the
    /// `unsafe` blocks carry no runtime precondition.
    #[derive(Clone, Copy)]
    pub(crate) struct Neon(pub int32x4_t);

    impl Simd4 for Neon {
        #[inline(always)]
        fn splat(v: i32) -> Self {
            // SAFETY: `neon` is statically enabled for this module (cfg above).
            Self(unsafe { vdupq_n_s32(v) })
        }
        #[inline(always)]
        fn from_array(a: [i32; 4]) -> Self {
            // SAFETY: `neon` enabled; `a` is a valid 4-element i32 array.
            Self(unsafe { vld1q_s32(a.as_ptr()) })
        }
        #[inline(always)]
        fn to_array(self) -> [i32; 4] {
            let mut out = [0i32; 4];
            // SAFETY: `neon` enabled; `out` has room for 4 i32.
            unsafe { vst1q_s32(out.as_mut_ptr(), self.0) };
            out
        }
        #[inline(always)]
        fn load(src: &[i32]) -> Self {
            assert!(src.len() >= 4, "Simd4::load needs 4 lanes");
            // SAFETY: `neon` enabled; the assert above guarantees `src` has
            // at least 4 readable i32, and vld1q_s32 has no alignment
            // requirement beyond that of i32.
            Self(unsafe { vld1q_s32(src.as_ptr()) })
        }
        #[inline(always)]
        fn store(self, dst: &mut [i32]) {
            assert!(dst.len() >= 4, "Simd4::store needs 4 lanes");
            // SAFETY: `neon` enabled; the assert above guarantees `dst` has
            // at least 4 writable i32.
            unsafe { vst1q_s32(dst.as_mut_ptr(), self.0) };
        }
        #[inline(always)]
        fn add(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vaddq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn sub(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vsubq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn mul(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vmulq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn min(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vminq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn max(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vmaxq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn abs(self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vabsq_s32(self.0) })
        }
        #[inline(always)]
        fn shr<const N: i32>(self) -> Self {
            // SAFETY: `neon` enabled; callers instantiate N in 1..=32.
            Self(unsafe { vshrq_n_s32::<N>(self.0) })
        }
        #[inline(always)]
        fn gt(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.  vcgtq_s32 yields an all-ones mask.
            Self(unsafe { vreinterpretq_s32_u32(vcgtq_s32(self.0, o.0)) })
        }
        #[inline(always)]
        fn and(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vandq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn or(self, o: Self) -> Self {
            // SAFETY: `neon` enabled.
            Self(unsafe { vorrq_s32(self.0, o.0) })
        }
        #[inline(always)]
        fn andnot(self, o: Self) -> Self {
            // vbicq_s32(a, b) == a & !b, so the operands are swapped here to
            // obtain (!self) & o.
            // SAFETY: `neon` enabled.
            Self(unsafe { vbicq_s32(o.0, self.0) })
        }
        #[inline(always)]
        fn any(self) -> bool {
            // SAFETY: `neon` enabled.
            unsafe { vmaxvq_u32(vreinterpretq_u32_s32(self.0)) != 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises one backend against plain `i32` arithmetic on a spread of
    /// values that covers negatives, zero and the magnitudes the AV1 kernels
    /// actually produce.
    fn check_backend<S: Simd4>(name: &str) {
        let vals: [i32; 12] = [0, 1, -1, 2, -2, 127, -128, 255, -255, 4096, -4096, 32767];
        for &a0 in &vals {
            for &b0 in &vals {
                let a = [a0, b0, a0.wrapping_sub(b0), b0 / 2];
                let b = [b0, a0, 1, -1];
                let va = S::from_array(a);
                let vb = S::from_array(b);

                let exp_add = [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
                assert_eq!(va.add(vb).to_array(), exp_add, "{name} add");
                let exp_sub = [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
                assert_eq!(va.sub(vb).to_array(), exp_sub, "{name} sub");
                let exp_mul = [
                    a[0].wrapping_mul(b[0]),
                    a[1].wrapping_mul(b[1]),
                    a[2].wrapping_mul(b[2]),
                    a[3].wrapping_mul(b[3]),
                ];
                assert_eq!(va.mul(vb).to_array(), exp_mul, "{name} mul");
                let exp_min = [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].min(b[2]),
                    a[3].min(b[3]),
                ];
                assert_eq!(va.min(vb).to_array(), exp_min, "{name} min");
                let exp_max = [
                    a[0].max(b[0]),
                    a[1].max(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ];
                assert_eq!(va.max(vb).to_array(), exp_max, "{name} max");
                let exp_abs = [a[0].abs(), a[1].abs(), a[2].abs(), a[3].abs()];
                assert_eq!(va.abs().to_array(), exp_abs, "{name} abs");
                let exp_shr3 = [a[0] >> 3, a[1] >> 3, a[2] >> 3, a[3] >> 3];
                assert_eq!(va.shr::<3>().to_array(), exp_shr3, "{name} shr3");
                let exp_shr1 = [a[0] >> 1, a[1] >> 1, a[2] >> 1, a[3] >> 1];
                assert_eq!(va.shr::<1>().to_array(), exp_shr1, "{name} shr1");

                let m = |x: bool| if x { -1i32 } else { 0 };
                let exp_gt = [
                    m(a[0] > b[0]),
                    m(a[1] > b[1]),
                    m(a[2] > b[2]),
                    m(a[3] > b[3]),
                ];
                assert_eq!(va.gt(vb).to_array(), exp_gt, "{name} gt");

                // select() must pick `a` exactly where gt() is true.
                let mask = va.gt(vb);
                let sel = S::select(mask, va, vb).to_array();
                for lane in 0..4 {
                    let want = if a[lane] > b[lane] { a[lane] } else { b[lane] };
                    assert_eq!(sel[lane], want, "{name} select lane {lane}");
                }
                assert_eq!(
                    mask.any(),
                    (0..4).any(|l| a[l] > b[l]),
                    "{name} any (gt mask)"
                );
                assert_eq!(
                    va.and(vb).to_array(),
                    [a[0] & b[0], a[1] & b[1], a[2] & b[2], a[3] & b[3]],
                    "{name} and"
                );
                assert_eq!(
                    va.or(vb).to_array(),
                    [a[0] | b[0], a[1] | b[1], a[2] | b[2], a[3] | b[3]],
                    "{name} or"
                );
                assert_eq!(
                    va.andnot(vb).to_array(),
                    [!a[0] & b[0], !a[1] & b[1], !a[2] & b[2], !a[3] & b[3]],
                    "{name} andnot"
                );
                let mut buf = [0i32; 6];
                buf[..4].copy_from_slice(&a);
                assert_eq!(S::load(&buf).to_array(), a, "{name} load");
                let mut outb = [9i32; 6];
                va.store(&mut outb);
                assert_eq!(&outb[..4], &a[..], "{name} store");
                assert_eq!(outb[4], 9, "{name} store overran");
                assert_eq!(
                    va.clamp(S::splat(-128), S::splat(127)).to_array(),
                    [
                        a[0].clamp(-128, 127),
                        a[1].clamp(-128, 127),
                        a[2].clamp(-128, 127),
                        a[3].clamp(-128, 127)
                    ],
                    "{name} clamp"
                );
            }
        }
        assert!(!S::splat(0).any(), "{name} any(0)");
        assert!(S::from_array([0, 0, 1, 0]).any(), "{name} any(one lane)");
    }

    #[test]
    fn portable_backend_matches_i32_semantics() {
        check_backend::<Portable>("portable");
    }

    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
    #[test]
    fn neon_backend_matches_i32_semantics() {
        check_backend::<Neon>("neon");
    }
}
