//! Data-parallel iteration helpers with a single-threaded `wasm32` fallback.
//!
//! `rayon` is a **native-only** dependency of this crate (see the
//! `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` section of
//! `Cargo.toml`). `wasm32-unknown-unknown` has no thread support, so a rayon
//! worker pool cannot be constructed there and any `par_iter` would fail at
//! run time rather than at compile time.
//!
//! Rather than sprinkling `#[cfg]` over every call site — which would let the
//! two branches drift — the crate funnels its parallel loops through the
//! handful of helpers below. Each has one signature (including the `Send` /
//! `Sync` bounds rayon needs, so the native branch cannot silently rot) and a
//! `#[cfg]`-selected body: rayon on native targets, plain sequential iteration
//! on `wasm32`. Results are collected in input order on both paths, so output
//! is identical regardless of target.

/// Maps `f` over `0..len`, collecting the results in index order.
pub(crate) fn map_range<T, F>(len: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize) -> T + Send + Sync,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        (0..len).into_par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        (0..len).map(f).collect()
    }
}

/// Maps `f` over `items`, collecting the results in slice order.
pub(crate) fn map_slice<T, U, F>(items: &[T], f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Send + Sync,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        items.par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        items.iter().map(f).collect()
    }
}

/// Maps `f` over `(index, item)` pairs, keeping the `Some` results in slice
/// order.
pub(crate) fn filter_map_enumerated<T, U, F>(items: &[T], f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(usize, &T) -> Option<U> + Send + Sync,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        items
            .par_iter()
            .enumerate()
            .filter_map(|(i, item)| f(i, item))
            .collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| f(i, item))
            .collect()
    }
}

/// Maps a fallible `f` over `items`, short-circuiting on the first error.
///
/// # Errors
///
/// Returns the first error produced by `f`. Which error surfaces first is
/// unspecified when several items fail, because the native path evaluates them
/// concurrently.
pub(crate) fn try_map_slice<T, U, E, F>(items: &[T], f: F) -> Result<Vec<U>, E>
where
    T: Sync,
    U: Send,
    E: Send,
    F: Fn(&T) -> Result<U, E> + Send + Sync,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        items.par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        items.iter().map(f).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{filter_map_enumerated, map_range, map_slice, try_map_slice};

    #[test]
    fn map_range_preserves_order() {
        assert_eq!(map_range(5, |i| i * i), vec![0, 1, 4, 9, 16]);
    }

    #[test]
    fn map_range_handles_empty() {
        assert!(map_range(0, |i: usize| i).is_empty());
    }

    #[test]
    fn map_slice_preserves_order() {
        let items = [3_u32, 1, 2];
        assert_eq!(map_slice(&items, |v| v * 10), vec![30, 10, 20]);
    }

    #[test]
    fn filter_map_enumerated_keeps_matching_items_in_order() {
        let items = [10_u32, 11, 12, 13];
        let kept = filter_map_enumerated(&items, |i, v| (i % 2 == 0).then_some(*v));
        assert_eq!(kept, vec![10, 12]);
    }

    #[test]
    fn try_map_slice_collects_ok() {
        let items = [1_u32, 2, 3];
        let out: Result<Vec<u32>, ()> = try_map_slice(&items, |v| Ok(v + 1));
        assert_eq!(out, Ok(vec![2, 3, 4]));
    }

    #[test]
    fn try_map_slice_propagates_error() {
        let items = [1_u32, 2, 3];
        let out: Result<Vec<u32>, &'static str> =
            try_map_slice(&items, |v| if *v == 2 { Err("boom") } else { Ok(*v) });
        assert_eq!(out, Err("boom"));
    }
}
