//! Synchronous driver for the tiny async futures produced by
//! `oximedia-container` / `oximedia-io`'s `MediaSource`/`Demuxer` traits
//! when they operate over an in-memory [`oximedia_io::source::MemorySource`].
//!
//! # Why this is sound
//!
//! `oximedia-container`'s demuxers are generic over `R: MediaSource`, and
//! `MediaSource` is an `async_trait` (so every method returns a boxed
//! future) purely so the same demuxer code can also drive real
//! network/file sources elsewhere in the workspace. `MemorySource::read`,
//! `write_all` and `seek` are plain synchronous memory operations wrapped
//! in `async fn` bodies with no internal `.await` point of their own, so
//! the future they produce always resolves to `Poll::Ready` on the very
//! first poll -- it can never observe a `Waker::wake()` because it never
//! returns `Poll::Pending` in the first place.
//!
//! Because Rust's async/await state machines run synchronously between
//! suspension points, this guarantee composes: a `MatroskaDemuxer::probe()`
//! call awaits a chain of `MemorySource::read()`/`seek()` futures, and
//! since none of those ever pend, the *entire* `probe()` future -- however
//! deep its internal `.await` chain -- resolves within a single top-level
//! `poll()`. [`poll_once`] exploits exactly this: it polls once with an
//! inert, no-op [`Waker`], and if that single poll is not enough to reach
//! `Poll::Ready`, something is suspending that this module's soundness
//! argument does not cover (e.g. a future built over a real I/O source by
//! caller error) -- at which point looping or spinning would only hang the
//! single-threaded WASM environment forever with no way to ever be woken.
//! [`poll_once`] deliberately never loops: a `Pending` first poll is
//! reported as an honest [`JsValue`] error instead.
//!
//! # No executor crate needed
//!
//! No `unsafe`, no `tokio`/`futures`-executor dependency: just
//! [`std::task::Waker::noop`] (stable since Rust 1.85; this workspace's
//! MSRV is 1.87) + [`std::task::Context`] + [`std::pin::pin!`].

use std::future::Future;
use std::task::{Context, Poll, Waker};

use oximedia_core::{OxiError, OxiResult};
use wasm_bindgen::JsValue;

/// Polls `fut` exactly once with a no-op [`Waker`] and returns its result.
///
/// # Errors
///
/// Returns an honest [`JsValue`] error -- never loops or spins -- if the
/// single poll does not resolve the future (i.e. it returns
/// `Poll::Pending`). See the module docs for why this should never happen
/// for futures built purely over [`oximedia_io::source::MemorySource`].
pub fn poll_once<F: Future>(fut: F) -> Result<F::Output, JsValue> {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut fut = std::pin::pin!(fut);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(value) => Ok(value),
        Poll::Pending => Err(crate::utils::js_err(
            "internal demuxer suspended — not supported in WASM",
        )),
    }
}

/// Polls a future that itself yields an [`OxiResult`], flattening the
/// (never-expected-in-practice, see module docs) "did not resolve in one
/// poll" case from [`poll_once`] into the very same `OxiResult` error
/// channel instead of a separate opaque `JsValue` layer.
///
/// `demuxer.rs`/`streaming_demuxer.rs` call this rather than [`poll_once`]
/// directly so every real container/demux error stays an inspectable
/// [`OxiError`] all the way up to the `#[wasm_bindgen]` public method
/// boundary (where [`crate::utils::to_js_error`] performs the final
/// conversion to `JsValue`). That is what makes error *messages* -- not
/// just the `Err`/`Ok` shape -- assertable from native `#[test]`s: a bare
/// `JsValue` is opaque outside `wasm32` (see [`crate::utils::js_err`]),
/// but `OxiError` is a real, inspectable enum on every target.
pub fn poll_oxi<T, F>(fut: F) -> OxiResult<T>
where
    F: Future<Output = OxiResult<T>>,
{
    poll_once(fut).unwrap_or_else(|_| {
        Err(OxiError::Unsupported(
            "internal demuxer suspended — not supported in WASM".to_string(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;

    /// A future that is always `Pending` -- never resolves, on any poll.
    ///
    /// Stands in for "something suspended that `poll_once`'s one-shot
    /// design cannot handle", exercising the honest-`Err` escape hatch.
    struct AlwaysPending;

    impl Future for AlwaysPending {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    #[test]
    fn ready_future_resolves_to_ok() {
        let result = poll_once(std::future::ready(42));
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn async_block_with_no_real_await_resolves_to_ok() {
        // An `async {}` block with no `.await` point desugars to a future
        // that is `Ready` on the first poll -- the same shape as
        // `MemorySource::read`/`seek`/`write_all`.
        let result = poll_once(async { 1 + 1 });
        assert_eq!(result, Ok(2));
    }

    #[test]
    fn pending_future_is_an_honest_err_not_a_hang() {
        let result = poll_once(AlwaysPending);
        assert!(
            result.is_err(),
            "a future that never resolves must not be silently treated as done"
        );
    }

    #[test]
    fn poll_oxi_passes_through_ready_ok_and_err() {
        let ok: OxiResult<i32> = poll_oxi(async { Ok(7) });
        assert_eq!(ok.expect("should be Ok"), 7);

        let err: OxiResult<i32> = poll_oxi(async { Err(OxiError::Eof) });
        assert!(matches!(err, Err(OxiError::Eof)));
    }

    /// A future whose `Output` is itself `OxiResult<T>` but that never
    /// resolves must still flatten into a real `OxiError`, not panic or
    /// silently vanish.
    struct AlwaysPendingOxi;

    impl Future for AlwaysPendingOxi {
        type Output = OxiResult<i32>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    #[test]
    fn poll_oxi_flattens_pending_into_an_oxierror() {
        let result = poll_oxi(AlwaysPendingOxi);
        assert!(result.is_err(), "a never-resolving future must be an Err");
    }
}
