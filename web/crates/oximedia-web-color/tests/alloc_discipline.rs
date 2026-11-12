// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Verifies the "no per-call allocation" discipline: after construction and
//! a warm-up call, `apply_rgba8` / `apply_rgba_f32` must not touch the heap.
//!
//! Uses a counting global allocator (test binary only; the library itself is
//! `#![forbid(unsafe_code)]`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use oximedia_web_color::{ColorPipeline, Lut3d, LutInterp, Primaries, ToneMapOperator};

struct CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations() -> usize {
    ALLOC_COUNT.load(Ordering::SeqCst)
}

fn full_pipeline() -> ColorPipeline {
    let mut p = ColorPipeline::new();
    p.set_exposure(0.7).expect("exposure");
    p.set_contrast(1.1).expect("contrast");
    p.set_saturation(1.2).expect("saturation");
    p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0)
        .expect("tone map");
    p.set_gamut(Primaries::Bt2020, Primaries::Bt709).expect("gamut");
    let lut = Lut3d::from_fn(33, |r, g, b| [r.sqrt(), g, b * b]).expect("lut");
    p.set_lut(lut, LutInterp::Tetrahedral);
    p
}

/// A single test (not two) so no sibling test thread can allocate into the
/// process-global counter during the measurement windows.
#[test]
fn apply_paths_do_not_allocate_after_warm_up() {
    // ── u8 path ──────────────────────────────────────────────────────────
    let mut p = full_pipeline();
    let src = vec![137u8; 256 * 256 * 4];
    let mut dst = vec![0u8; src.len()];

    // Warm-up (nothing should allocate even here, but be conservative).
    p.apply_rgba8(&src, &mut dst).expect("warm-up");

    let before = allocations();
    for _ in 0..3 {
        p.apply_rgba8(&src, &mut dst).expect("apply");
    }
    let after = allocations();
    assert_eq!(
        after - before,
        0,
        "apply_rgba8 must not allocate per call (saw {} allocations)",
        after - before
    );

    // ── f32 path ─────────────────────────────────────────────────────────
    let fsrc = vec![0.42f32; 128 * 128 * 4];
    let mut fdst = vec![0.0f32; fsrc.len()];

    p.apply_rgba_f32(&fsrc, &mut fdst).expect("warm-up");

    let before = allocations();
    for _ in 0..3 {
        p.apply_rgba_f32(&fsrc, &mut fdst).expect("apply");
    }
    let after = allocations();
    assert_eq!(
        after - before,
        0,
        "apply_rgba_f32 must not allocate per call (saw {} allocations)",
        after - before
    );
}
