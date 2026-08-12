//! Arithmetic shared by more than one platform backend.
//!
//! Nothing here talks to an operating system. It exists because two backends
//! independently need the same exact-rational reduction and duplicating it
//! would let the two copies drift: a frame rate reduced by one backend and not
//! by another produces two [`CaptureFormat`](crate::CaptureFormat) values that
//! describe the same mode but compare unequal, which silently defeats the
//! duplicate filter in enumeration.
//!
//! The module is compiled for the targets whose backends use it, and under
//! `cfg(test)` everywhere so that the host test suite covers it regardless of
//! which backend the host has. A backend added later must add its `target_os`
//! to the `cfg` in [`super`].

/// Greatest common divisor, for reducing a frame rate to lowest terms.
///
/// The classic Euclidean algorithm. `gcd(0, n) == n` and `gcd(0, 0) == 0`,
/// which is why every caller clamps the divisor to at least one before
/// dividing.
pub(crate) const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcd_reduces_the_rates_capture_apis_actually_report() {
        // AVFoundation's microsecond timescale, Media Foundation's plain
        // integer rates, and NTSC, which is already in lowest terms.
        assert_eq!(gcd(30_000_000, 1_000_000), 1_000_000);
        assert_eq!(gcd(30, 1), 1);
        assert_eq!(gcd(30_000, 1001), 1);
        assert_eq!(gcd(60_000, 2002), 2);
    }

    #[test]
    fn gcd_is_symmetric() {
        for (a, b) in [(30_u32, 12_u32), (1001, 30_000), (7, 13), (48, 18)] {
            assert_eq!(gcd(a, b), gcd(b, a), "gcd({a}, {b})");
        }
    }

    #[test]
    fn gcd_with_zero_is_the_other_operand() {
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(5, 0), 5);
        assert_eq!(gcd(0, 0), 0, "the degenerate case every caller clamps");
    }

    #[test]
    fn dividing_by_the_gcd_leaves_a_reduced_pair() {
        for (a, b) in [(30_000_000_u32, 1_000_000_u32), (60_000, 2002), (48, 18)] {
            let divisor = gcd(a, b).max(1);
            assert_eq!(gcd(a / divisor, b / divisor), 1, "{a}/{b} not reduced");
        }
    }
}
