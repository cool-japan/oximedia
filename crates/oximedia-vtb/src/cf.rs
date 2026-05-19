//! CoreFoundation refcounting via Rust's `Drop`.
//!
//! Every Apple Core* framework (CoreFoundation, CoreMedia, CoreVideo,
//! VideoToolbox) inherits from `CFType`, which is reference-counted via
//! `CFRetain` / `CFRelease`. APIs that **create** an object return a
//! reference with +1 retain count owned by the caller, and APIs that
//! **return** an existing object return a borrowed reference that you
//! must `CFRetain` if you want to outlive the call.
//!
//! This module provides one zero-overhead wrapper, [`CfOwned<T>`], that
//! captures the +1-retain invariant in the type system: the value is
//! `CFRelease`d when the wrapper is dropped, and cloning calls
//! `CFRetain` to keep both copies valid.

use std::marker::PhantomData;
use std::ptr::NonNull;

use oximedia_vtb_sys::{CFRelease, CFRetain};

/// An owned `+1`-retained reference to a CoreFoundation-based object.
///
/// `T` is the bindgen-generated opaque type (e.g. `CMSampleBuffer`,
/// `CVPixelBuffer`, `VTDecompressionSession`). The pointer is treated
/// as type-erased CFTypeRef internally — every Apple opaque type can
/// be passed to `CFRetain` / `CFRelease`.
#[derive(Debug)]
pub struct CfOwned<T> {
    ptr: NonNull<T>,
    _marker: PhantomData<T>,
}

impl<T> CfOwned<T> {
    /// Adopt a raw pointer that was returned with `+1` retain count.
    ///
    /// # Safety
    ///
    /// `ptr` must be either null or a valid `+1`-retained pointer to a
    /// CFType-derived object of type `T`. Passing a borrowed reference
    /// without an explicit `CFRetain` will cause a use-after-free when
    /// this wrapper is dropped.
    pub unsafe fn from_create(ptr: *mut T) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self {
            ptr,
            _marker: PhantomData,
        })
    }

    /// Adopt a raw pointer that was returned **without** a retain — for
    /// example by a `*Get*` accessor that returns a borrowed reference.
    /// We bump the retain count to take shared ownership.
    ///
    /// # Safety
    ///
    /// `ptr` must be either null or a currently-live CFType pointer
    /// of type `T`.
    #[allow(dead_code)]
    pub unsafe fn from_get(ptr: *mut T) -> Option<Self> {
        let ptr = NonNull::new(ptr)?;
        // SAFETY: caller asserts ptr is currently live, so retaining is sound.
        unsafe {
            CFRetain(ptr.as_ptr().cast());
        }
        Some(Self {
            ptr,
            _marker: PhantomData,
        })
    }

    /// Borrow the underlying pointer for an FFI call.
    ///
    /// The returned pointer is valid for as long as `self` is.
    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }
}

impl<T> Clone for CfOwned<T> {
    fn clone(&self) -> Self {
        // SAFETY: `self.ptr` is a live CFType (invariant of `CfOwned`),
        // so CFRetain is sound.
        unsafe {
            CFRetain(self.ptr.as_ptr().cast());
        }
        Self {
            ptr: self.ptr,
            _marker: PhantomData,
        }
    }
}

impl<T> Drop for CfOwned<T> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is a live CFType with `+1` retain held by
        // this `CfOwned`. CFRelease consumes that retain.
        unsafe {
            CFRelease(self.ptr.as_ptr().cast());
        }
    }
}

// CFType objects are documented as thread-safe for `CFRetain`/`CFRelease`
// and most accessor calls; the higher-level wrappers further restrict
// usage where the underlying object isn't (e.g. session objects).
unsafe impl<T> Send for CfOwned<T> {}
unsafe impl<T> Sync for CfOwned<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_vtb_sys::{CFStringCreateWithCString, CFStringGetLength};

    /// CoreFoundation UTF-8 string encoding constant. Not in the bindgen
    /// allowlist as a named symbol, so we use the well-known value.
    const K_CFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[test]
    fn create_and_drop_runs_release_exactly_once() {
        let raw = b"oximedia-vtb-cf-test\0";
        // SAFETY: passing null allocator + valid C-string + valid encoding.
        let s = unsafe {
            CFStringCreateWithCString(
                std::ptr::null(),
                raw.as_ptr().cast(),
                K_CFSTRING_ENCODING_UTF8,
            )
        };
        // SAFETY: CFStringCreateWithCString returns +1 retain when non-null.
        let owned = unsafe { CfOwned::from_create(s.cast_mut()) }.expect("CFString created");
        // Length check works while the object is alive.
        let len = unsafe { CFStringGetLength(owned.as_ptr()) };
        assert_eq!(len as usize, raw.len() - 1);
        // owned drops here → CFRelease is called exactly once.
    }

    #[test]
    fn clone_doubles_retain_so_both_can_live() {
        let raw = b"clone-test\0";
        let s = unsafe {
            CFStringCreateWithCString(
                std::ptr::null(),
                raw.as_ptr().cast(),
                K_CFSTRING_ENCODING_UTF8,
            )
        };
        let owned = unsafe { CfOwned::from_create(s.cast_mut()) }.expect("CFString created");
        let cloned = owned.clone();
        // Both wrappers are valid here — if CFRetain wasn't called on
        // clone, dropping one would invalidate the other and the second
        // read would crash. The fact that we get here without trapping
        // is the test.
        assert_eq!(
            unsafe { CFStringGetLength(owned.as_ptr()) },
            unsafe { CFStringGetLength(cloned.as_ptr()) },
        );
        drop(owned);
        // cloned must still be readable after owned's drop.
        let len_after = unsafe { CFStringGetLength(cloned.as_ptr()) };
        assert_eq!(len_after as usize, raw.len() - 1);
    }

    #[test]
    fn from_create_returns_none_for_null() {
        let owned: Option<CfOwned<i32>> = unsafe { CfOwned::from_create(std::ptr::null_mut()) };
        assert!(owned.is_none());
    }
}
