// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! 3D LUT (`f32`, flattened RGB lattice) with trilinear **and** tetrahedral
//! interpolation.
//!
//! The tetrahedral kernel is a port of the Sakamoto decomposition from
//! `crates/oximedia-colormgmt/src/lut_interp.rs` (`TetrahedralInterpolator`).
//! The lattice storage order is **R-fastest** (the Adobe/ffmpeg `.cube`
//! convention, matching `crates/oximedia-lut/src/formats/cube.rs`) — *not*
//! the B-fastest order used by `oximedia-colormgmt`'s `GradingLut3D` export,
//! which is a known cross-compatibility bug this crate does not copy.

use crate::error::ColorError;

/// Minimum supported lattice size per axis.
pub const MIN_LUT_SIZE: usize = 2;
/// Maximum supported lattice size per axis (129³ ≈ 25 MB of f32 RGB).
pub const MAX_LUT_SIZE: usize = 129;

/// Interpolation kernel selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutInterp {
    /// Classic 8-corner trilinear interpolation.
    Trilinear,
    /// Sakamoto 6-tetrahedron interpolation (higher colour accuracy along
    /// the neutral axis; the film-industry default).
    Tetrahedral,
}

impl LutInterp {
    /// Parses an interpolation name (`"trilinear"` or `"tetrahedral"`,
    /// ASCII case-insensitive).
    ///
    /// # Errors
    /// Returns [`ColorError::UnknownName`] for anything else.
    pub fn parse(name: &str) -> Result<Self, ColorError> {
        match name.to_ascii_lowercase().as_str() {
            "trilinear" => Ok(Self::Trilinear),
            "tetrahedral" => Ok(Self::Tetrahedral),
            _ => Err(ColorError::UnknownName {
                kind: "LUT interpolation",
                name: name.to_string(),
            }),
        }
    }

    /// Canonical lowercase name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Trilinear => "trilinear",
            Self::Tetrahedral => "tetrahedral",
        }
    }
}

/// A 3D colour LUT: `size³` RGB triplets on a uniform lattice over
/// `[domain_min, domain_max]`.
///
/// Storage is a flattened `Vec<f32>` in R-fastest order:
/// `index(r, g, b) = ((b·size + g)·size + r) · 3`.
#[derive(Clone, Debug)]
pub struct Lut3d {
    size: usize,
    data: Vec<f32>,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    /// Precomputed `1 / (domain_max - domain_min)` per channel.
    inv_range: [f32; 3],
    title: Option<String>,
}

impl Lut3d {
    /// Creates a LUT from raw data (R-fastest order, length `size³ × 3`)
    /// with the default `[0, 1]` domain.
    ///
    /// # Errors
    /// [`ColorError::LutSize`] if `size` is outside `2..=129`;
    /// [`ColorError::LutDataLength`] on a length mismatch;
    /// [`ColorError::LutNonFinite`] if any entry is NaN/infinite.
    pub fn new(size: usize, data: Vec<f32>) -> Result<Self, ColorError> {
        Self::with_domain(size, data, [0.0; 3], [1.0; 3], None)
    }

    /// Creates a LUT with an explicit domain and optional title.
    ///
    /// # Errors
    /// As [`Lut3d::new`], plus [`ColorError::LutDomain`] if any channel has
    /// `domain_max ≤ domain_min` or a non-finite bound.
    pub fn with_domain(
        size: usize,
        data: Vec<f32>,
        domain_min: [f32; 3],
        domain_max: [f32; 3],
        title: Option<String>,
    ) -> Result<Self, ColorError> {
        if !(MIN_LUT_SIZE..=MAX_LUT_SIZE).contains(&size) {
            return Err(ColorError::LutSize { size });
        }
        let expected = size * size * size * 3;
        if data.len() != expected {
            return Err(ColorError::LutDataLength {
                expected,
                actual: data.len(),
            });
        }
        if data.iter().any(|v| !v.is_finite()) {
            return Err(ColorError::LutNonFinite);
        }
        let mut inv_range = [0.0f32; 3];
        for i in 0..3 {
            let (lo, hi) = (domain_min[i], domain_max[i]);
            if !lo.is_finite() || !hi.is_finite() || hi <= lo {
                return Err(ColorError::LutDomain);
            }
            inv_range[i] = 1.0 / (hi - lo);
        }
        Ok(Self {
            size,
            data,
            domain_min,
            domain_max,
            inv_range,
            title,
        })
    }

    /// Creates an identity LUT of the given size.
    ///
    /// # Errors
    /// [`ColorError::LutSize`] if `size` is outside `2..=129`.
    pub fn identity(size: usize) -> Result<Self, ColorError> {
        Self::from_fn(size, |r, g, b| [r, g, b])
    }

    /// Builds a LUT by sampling `f` at every lattice point (arguments in
    /// `[0, 1]`, R-fastest iteration).
    ///
    /// # Errors
    /// [`ColorError::LutSize`] if `size` is outside `2..=129`;
    /// [`ColorError::LutNonFinite`] if `f` produces NaN/infinite values.
    pub fn from_fn<F>(size: usize, mut f: F) -> Result<Self, ColorError>
    where
        F: FnMut(f32, f32, f32) -> [f32; 3],
    {
        if !(MIN_LUT_SIZE..=MAX_LUT_SIZE).contains(&size) {
            return Err(ColorError::LutSize { size });
        }
        let inv_scale = 1.0 / (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size * 3);
        for b in 0..size {
            let fb = b as f32 * inv_scale;
            for g in 0..size {
                let fg = g as f32 * inv_scale;
                for r in 0..size {
                    let fr = r as f32 * inv_scale;
                    let out = f(fr, fg, fb);
                    data.extend_from_slice(&out);
                }
            }
        }
        Self::new(size, data)
    }

    /// Lattice size per axis.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Raw flattened data (R-fastest order).
    #[must_use]
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Input domain minimum per channel.
    #[must_use]
    pub const fn domain_min(&self) -> [f32; 3] {
        self.domain_min
    }

    /// Input domain maximum per channel.
    #[must_use]
    pub const fn domain_max(&self) -> [f32; 3] {
        self.domain_max
    }

    /// LUT title (from/for the `.cube` `TITLE` line).
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Sets the title.
    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    /// Fetches the RGB triplet at lattice point `(ri, gi, bi)`.
    ///
    /// Callers guarantee indices `< size`; out-of-bounds access returns
    /// black rather than panicking (defence in depth, no `unwrap`).
    #[inline]
    fn cell(&self, ri: usize, gi: usize, bi: usize) -> [f32; 3] {
        let idx = ((bi * self.size + gi) * self.size + ri) * 3;
        match self.data.get(idx..idx + 3) {
            Some(s) => [s[0], s[1], s[2]],
            None => [0.0; 3],
        }
    }

    /// Fetches the r-adjacent lattice pair `(ri, gi, bi)` and
    /// `(ri+1, gi, bi)` with a single bounds check (R-fastest storage makes
    /// them contiguous — the reason that order also wins on the hot path).
    #[inline]
    fn cell_pair_r(&self, ri: usize, gi: usize, bi: usize) -> ([f32; 3], [f32; 3]) {
        let idx = ((bi * self.size + gi) * self.size + ri) * 3;
        match self.data.get(idx..idx + 6) {
            Some(s) => ([s[0], s[1], s[2]], [s[3], s[4], s[5]]),
            None => ([0.0; 3], [0.0; 3]),
        }
    }

    /// Maps an input coordinate to `(base_index, fraction)` on one axis,
    /// applying the domain normalisation. NaN inputs resolve to 0.
    #[inline]
    fn locate(&self, v: f32, axis: usize) -> (usize, f32) {
        let v = if v.is_finite() { v } else { 0.0 };
        let t = ((v - self.domain_min[axis]) * self.inv_range[axis]).clamp(0.0, 1.0);
        let pos = t * (self.size - 1) as f32;
        let i0 = (pos as usize).min(self.size - 2);
        (i0, pos - i0 as f32)
    }

    /// Samples the LUT with 8-corner trilinear interpolation.
    ///
    /// Inputs outside the domain are clamped; NaN resolves to the domain
    /// minimum.
    #[inline]
    #[must_use]
    pub fn sample_trilinear(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let (r0, rf) = self.locate(r, 0);
        let (g0, gf) = self.locate(g, 1);
        let (b0, bf) = self.locate(b, 2);
        let (g1, b1) = (g0 + 1, b0 + 1);

        let (c000, c100) = self.cell_pair_r(r0, g0, b0);
        let (c010, c110) = self.cell_pair_r(r0, g1, b0);
        let (c001, c101) = self.cell_pair_r(r0, g0, b1);
        let (c011, c111) = self.cell_pair_r(r0, g1, b1);

        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let x00 = c000[i] + rf * (c100[i] - c000[i]);
            let x10 = c010[i] + rf * (c110[i] - c010[i]);
            let x01 = c001[i] + rf * (c101[i] - c001[i]);
            let x11 = c011[i] + rf * (c111[i] - c011[i]);
            let y0 = x00 + gf * (x10 - x00);
            let y1 = x01 + gf * (x11 - x01);
            out[i] = y0 + bf * (y1 - y0);
        }
        out
    }

    /// Samples the LUT with Sakamoto tetrahedral interpolation (ported from
    /// `oximedia-colormgmt::lut_interp::interpolate_3d`).
    ///
    /// Inputs outside the domain are clamped; NaN resolves to the domain
    /// minimum.
    #[inline]
    #[must_use]
    pub fn sample_tetrahedral(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let (r0, rf) = self.locate(r, 0);
        let (g0, gf) = self.locate(g, 1);
        let (b0, bf) = self.locate(b, 2);
        let (r1, g1, b1) = (r0 + 1, g0 + 1, b0 + 1);

        // Each tetrahedron needs four corners; one r-adjacent pair per
        // branch is fetched with a single bounds check.
        if rf >= gf {
            if gf >= bf {
                // T1: rf ≥ gf ≥ bf
                let (c000, c100) = self.cell_pair_r(r0, g0, b0);
                let c110 = self.cell(r1, g1, b0);
                let c111 = self.cell(r1, g1, b1);
                interp4(c000, c100, c110, c111, rf, gf, bf)
            } else if rf >= bf {
                // T2: rf ≥ bf > gf
                let (c000, c100) = self.cell_pair_r(r0, g0, b0);
                let c101 = self.cell(r1, g0, b1);
                let c111 = self.cell(r1, g1, b1);
                interp4(c000, c100, c101, c111, rf, bf, gf)
            } else {
                // T3: bf > rf ≥ gf
                let (c001, c101) = self.cell_pair_r(r0, g0, b1);
                let c000 = self.cell(r0, g0, b0);
                let c111 = self.cell(r1, g1, b1);
                interp4(c000, c001, c101, c111, bf, rf, gf)
            }
        } else if bf >= gf {
            // T4: bf ≥ gf > rf
            let (c011, c111) = self.cell_pair_r(r0, g1, b1);
            let c000 = self.cell(r0, g0, b0);
            let c001 = self.cell(r0, g0, b1);
            interp4(c000, c001, c011, c111, bf, gf, rf)
        } else if bf > rf {
            // T5: gf > bf > rf
            let (c011, c111) = self.cell_pair_r(r0, g1, b1);
            let c000 = self.cell(r0, g0, b0);
            let c010 = self.cell(r0, g1, b0);
            interp4(c000, c010, c011, c111, gf, bf, rf)
        } else {
            // T6: gf > rf ≥ bf
            let (c010, c110) = self.cell_pair_r(r0, g1, b0);
            let c000 = self.cell(r0, g0, b0);
            let c111 = self.cell(r1, g1, b1);
            interp4(c000, c010, c110, c111, gf, rf, bf)
        }
    }

    /// Samples with the requested kernel.
    #[inline]
    #[must_use]
    pub fn sample(&self, interp: LutInterp, r: f32, g: f32, b: f32) -> [f32; 3] {
        match interp {
            LutInterp::Trilinear => self.sample_trilinear(r, g, b),
            LutInterp::Tetrahedral => self.sample_tetrahedral(r, g, b),
        }
    }
}

/// Sakamoto barycentric blend along the path `v0 → v1 → v2 → v3`:
/// `v0 + w1(v1−v0) + w2(v2−v1) + w3(v3−v2)`.
#[inline]
fn interp4(
    v0: [f32; 3],
    v1: [f32; 3],
    v2: [f32; 3],
    v3: [f32; 3],
    w1: f32,
    w2: f32,
    w3: f32,
) -> [f32; 3] {
    [
        v0[0] + w1 * (v1[0] - v0[0]) + w2 * (v2[0] - v1[0]) + w3 * (v3[0] - v2[0]),
        v0[1] + w1 * (v1[1] - v0[1]) + w2 * (v2[1] - v1[1]) + w3 * (v3[1] - v2[1]),
        v0[2] + w1 * (v1[2] - v0[2]) + w2 * (v2[2] - v1[2]) + w3 * (v3[2] - v2[2]),
    ]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn identity_lut_both_interps_within_1e_5() {
        let lut = Lut3d::identity(17).expect("identity");
        for i in 0..=20 {
            for j in 0..=20 {
                let r = i as f32 / 20.0;
                let g = j as f32 / 20.0;
                let b = (i as f32 * 0.31 + j as f32 * 0.17) % 1.0;
                for interp in [LutInterp::Trilinear, LutInterp::Tetrahedral] {
                    let out = lut.sample(interp, r, g, b);
                    assert!(
                        approx(out[0], r, 1e-5)
                            && approx(out[1], g, 1e-5)
                            && approx(out[2], b, 1e-5),
                        "{}: identity({r},{g},{b}) = {out:?}",
                        interp.name()
                    );
                }
            }
        }
    }

    #[test]
    fn tetrahedral_equals_trilinear_on_lattice_points() {
        let size = 9;
        // A deliberately non-linear LUT.
        let lut = Lut3d::from_fn(size, |r, g, b| {
            [r * r, (g + b) * 0.5, (r * 0.2 + b * 0.8).sqrt()]
        })
        .expect("from_fn");
        let scale = (size - 1) as f32;
        for bi in 0..size {
            for gi in 0..size {
                for ri in 0..size {
                    let (r, g, b) = (ri as f32 / scale, gi as f32 / scale, bi as f32 / scale);
                    let tri = lut.sample_trilinear(r, g, b);
                    let tet = lut.sample_tetrahedral(r, g, b);
                    for k in 0..3 {
                        assert!(
                            approx(tri[k], tet[k], 1e-5),
                            "lattice ({ri},{gi},{bi}) ch{k}: {} vs {}",
                            tri[k],
                            tet[k]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn out_of_range_inputs_clamp() {
        let lut = Lut3d::identity(5).expect("identity");
        for interp in [LutInterp::Trilinear, LutInterp::Tetrahedral] {
            let lo = lut.sample(interp, -3.0, -0.1, -1e30);
            let hi = lut.sample(interp, 3.0, 1.1, 1e30);
            assert!(approx(lo[0], 0.0, 1e-6) && approx(lo[1], 0.0, 1e-6));
            assert!(approx(hi[0], 1.0, 1e-6) && approx(hi[1], 1.0, 1e-6));
        }
    }

    #[test]
    fn nan_input_does_not_panic() {
        let lut = Lut3d::identity(5).expect("identity");
        for interp in [LutInterp::Trilinear, LutInterp::Tetrahedral] {
            let out = lut.sample(interp, f32::NAN, 0.5, f32::INFINITY);
            for v in out {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn custom_domain_remaps_input() {
        // Identity lattice over the domain [-1, 1]: sampling at 0 must give
        // the lattice midpoint 0.5-ish output (the raw lattice values).
        let lut = Lut3d::with_domain(
            3,
            Lut3d::identity(3).expect("id").data().to_vec(),
            [-1.0; 3],
            [1.0; 3],
            None,
        )
        .expect("domain lut");
        let out = lut.sample_trilinear(0.0, 0.0, 0.0);
        assert!(approx(out[0], 0.5, 1e-6), "domain midpoint: {:?}", out);
    }

    #[test]
    fn size_bounds_are_enforced() {
        assert!(matches!(Lut3d::identity(1), Err(ColorError::LutSize { .. })));
        assert!(matches!(Lut3d::identity(130), Err(ColorError::LutSize { .. })));
        assert!(matches!(Lut3d::identity(0), Err(ColorError::LutSize { .. })));
        assert!(Lut3d::identity(2).is_ok());
        // 129 is allowed but big (~25 MB); construct via from_fn to confirm.
        assert!(Lut3d::from_fn(129, |r, g, b| [r, g, b]).is_ok());
    }

    #[test]
    fn data_length_is_validated() {
        assert!(matches!(
            Lut3d::new(2, vec![0.0; 23]),
            Err(ColorError::LutDataLength { .. })
        ));
        assert!(Lut3d::new(2, vec![0.0; 24]).is_ok());
    }

    #[test]
    fn non_finite_data_is_rejected() {
        let mut data = vec![0.0f32; 24];
        data[7] = f32::NAN;
        assert!(matches!(Lut3d::new(2, data), Err(ColorError::LutNonFinite)));
        let mut data = vec![0.0f32; 24];
        data[0] = f32::INFINITY;
        assert!(matches!(Lut3d::new(2, data), Err(ColorError::LutNonFinite)));
    }

    #[test]
    fn degenerate_domain_is_rejected() {
        let data = vec![0.0f32; 24];
        assert!(matches!(
            Lut3d::with_domain(2, data.clone(), [0.0; 3], [0.0; 3], None),
            Err(ColorError::LutDomain)
        ));
        assert!(matches!(
            Lut3d::with_domain(2, data, [0.0; 3], [1.0, 1.0, f32::NAN], None),
            Err(ColorError::LutDomain)
        ));
    }

    #[test]
    fn r_fastest_storage_order() {
        // Build a LUT where the output encodes the lattice coordinate, then
        // verify that data()[0..3] is (0,0,0) and data()[3..6] steps R first.
        let lut = Lut3d::from_fn(3, |r, g, b| [r, g, b]).expect("from_fn");
        let d = lut.data();
        assert!(approx(d[0], 0.0, 0.0) && approx(d[1], 0.0, 0.0) && approx(d[2], 0.0, 0.0));
        // Second entry: r advances first (R-fastest).
        assert!(approx(d[3], 0.5, 1e-6), "second entry must step R: {}", d[3]);
        assert!(approx(d[4], 0.0, 0.0) && approx(d[5], 0.0, 0.0));
    }

    #[test]
    fn interp_parse() {
        assert_eq!(LutInterp::parse("trilinear"), Ok(LutInterp::Trilinear));
        assert_eq!(LutInterp::parse("TETRAHEDRAL"), Ok(LutInterp::Tetrahedral));
        assert!(LutInterp::parse("nearest").is_err());
    }
}
