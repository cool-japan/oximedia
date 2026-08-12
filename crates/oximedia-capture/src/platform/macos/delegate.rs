//! The `AVCaptureVideoDataOutputSampleBufferDelegate` that publishes frames.
//!
//! AVFoundation pushes: it calls
//! `-captureOutput:didOutputSampleBuffer:fromConnection:` on a dispatch queue
//! of the client's choosing, once per frame. This module defines the
//! Objective-C class that receives those calls and turns each sample buffer
//! into a [`CaptureFrame`] on the crate's delivery queue.
//!
//! # Lifetime of the sink pointer
//!
//! The ivars hold a raw `*const FrameSink`. That is not a shortcut around
//! borrow checking for its own sake — it is the only way to keep
//! [`FrameSink::deliver`] as the single place where [`crate::DropPolicy`],
//! clock normalization and statistics are applied, which is the contract
//! `backend.rs` is built on. The alternatives were weighed and are worse:
//!
//! * handing frames to the capture thread over a second channel forks the
//!   drop-policy decision (the *delegate* would have to decide what to do when
//!   that channel is full), and books this crate's own back-pressure as
//!   [`CaptureStats::device_dropped`](crate::CaptureStats::device_dropped),
//!   which is reserved for losses upstream of this crate;
//! * an unbounded hand-off channel grows without limit under
//!   [`DropPolicy::Block`](crate::DropPolicy::Block) with a stalled consumer.
//!
//! [`FrameSink`] is already `Send + Sync` — a `crossbeam` sender/receiver pair,
//! an `Arc<StatsInner>` of atomics, a `parking_lot::Mutex` and two `Copy`
//! fields — so *sharing* it across the dispatch queue thread is sound. Only its
//! lifetime needs an argument, and that argument is made by
//! [`super::avf::SessionGuard`], which on drop, in order:
//!
//! 1. raises the [`StopSignal`](crate::backend::StopSignal), so a callback
//!    parked inside a blocking send gives up within one poll interval;
//! 2. calls `-stopRunning`, which blocks until the session has stopped;
//! 3. clears the delegate with `-setSampleBufferDelegate:nil queue:nil`, so no
//!    further callback can be scheduled;
//! 4. runs an empty block *synchronously* on the callback queue. The queue is
//!    serial, so that block cannot start until any in-flight callback body has
//!    returned.
//!
//! The guard is dropped before `CaptureRunner::run` returns, and `run` borrows
//! the `FrameSink` for its whole body, so no callback body can execute after
//! the sink's lifetime ends. Deallocation of the delegate object itself may be
//! deferred by an autorelease pool; that is harmless, because a deallocated-but
//! not-yet-freed delegate receives no messages.
//!
//! # Callback rules
//!
//! * No Rust lock is held across an Objective-C message send. All framework
//!   work (locking the pixel buffer, copying planes) finishes before
//!   [`FrameSink::deliver`] takes the sink's mutex.
//! * Nothing panics. Every failure path increments
//!   [`CaptureStats::errors`](crate::CaptureStats::errors) through
//!   [`FrameSink::report_transient`] and returns; a panic unwinding into
//!   Objective-C frames is undefined behaviour.
//! * The sequence counter advances only for frames that are actually
//!   published. Counting skipped frames would open a gap that
//!   [`FrameSink::deliver`] would attribute to the *device*, turning this
//!   crate's own error into a fabricated hardware fault.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSObject, NSObjectProtocol};

use crate::backend::FrameSink;
use crate::device::CaptureFormat;
use crate::error::CaptureError;
use crate::frame::{CaptureFrame, FramePayload, TimestampSource};

use super::{cmtime_to_duration, pixel};

// ── Sink handle ──────────────────────────────────────────────────────────────

/// A borrow of the session's [`FrameSink`], stored in Objective-C ivars.
///
/// See the module documentation for the lifetime argument. The pointer is
/// obtained from a live `&FrameSink` and is never null, never offset and never
/// written through.
struct SinkHandle(*const FrameSink);

// SAFETY: `FrameSink` is `Send + Sync` (crossbeam channel endpoints, an
// `Arc<StatsInner>` of atomics, a `parking_lot::Mutex` and two `Copy` fields),
// so a shared reference to one may be used from any thread. `SinkHandle` adds
// nothing but a lifetime erasure, and the module documentation states the
// invariant that keeps the referent alive for as long as the handle can be
// used.
unsafe impl Send for SinkHandle {}
// SAFETY: as above — `&FrameSink` is `Send`, therefore sharing `SinkHandle`
// between threads only ever produces `&FrameSink`s, which is sound.
unsafe impl Sync for SinkHandle {}

impl SinkHandle {
    /// Borrow the sink.
    fn sink(&self) -> &FrameSink {
        // SAFETY: the pointer came from `std::ptr::from_ref` on a live
        // `&FrameSink` in `CaptureRunner::run`, and the teardown sequence
        // documented at the module level guarantees that no caller of this
        // method can still be running when that borrow ends.
        unsafe { &*self.0 }
    }
}

// ── Ivars ────────────────────────────────────────────────────────────────────

/// Everything the callback needs, none of it borrowed from Objective-C.
///
/// `pub(crate)` only because `define_class!` names it in the generated
/// `DefinedClass` impl; nothing outside this module constructs or reads one.
pub(crate) struct DelegateIvars {
    /// Where published frames go.
    sink: SinkHandle,
    /// The mode the device was configured for; every frame is checked against
    /// it and none is delivered at a different size.
    format: CaptureFormat,
    /// Device id, for error messages.
    device: String,
    /// Host clock origin, sampled once when the delegate is built.
    epoch: Instant,
    /// Frames published so far. Interior mutability is required because the
    /// callback only ever receives `&self`.
    sequence: AtomicU64,
}

define_class!(
    // SAFETY:
    // - `NSObject` imposes no subclassing requirements.
    // - `SampleDelegate` does not implement `Drop`; the macro's generated
    //   `dealloc` drops the ivars, which is what frees the `String`.
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    pub(crate) struct SampleDelegate;

    unsafe impl NSObjectProtocol for SampleDelegate {}

    // SAFETY: both selectors below are declared by this protocol with exactly
    // these signatures, and both are `@optional`, so implementing them is
    // sufficient to conform.
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for SampleDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn capture_output_did_output_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            self.publish(sample_buffer);
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        fn capture_output_did_drop_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            _sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // AVFoundation dropped this frame before this crate ever saw its
            // samples — the same category as a V4L2 sequence gap, and
            // deliberately not counted as one of this crate's own queue drops.
            self.ivars().sink.sink().report_device_dropped(1);
        }
    }
);

impl SampleDelegate {
    /// Build a delegate that publishes into `sink`.
    ///
    /// # Safety
    ///
    /// `sink` must outlive every callback the returned delegate can receive.
    /// The caller guarantees this with [`super::avf::SessionGuard`]; see the
    /// module documentation.
    pub(crate) unsafe fn new(
        sink: &FrameSink,
        format: CaptureFormat,
        device: String,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars {
            sink: SinkHandle(std::ptr::from_ref(sink)),
            format,
            device,
            epoch: Instant::now(),
            sequence: AtomicU64::new(0),
        });
        // SAFETY: `-init` is `NSObject`'s designated initializer and the ivars
        // have already been set, which is the order `define_class!` requires.
        unsafe { msg_send![super(this), init] }
    }

    /// Type-erase to the protocol object `-setSampleBufferDelegate:queue:`
    /// wants.
    pub(crate) fn as_protocol(
        &self,
    ) -> &ProtocolObject<dyn AVCaptureVideoDataOutputSampleBufferDelegate> {
        ProtocolObject::from_ref(self)
    }

    /// Turn one sample buffer into a delivered frame.
    ///
    /// Never panics and never propagates: a frame this method cannot build is
    /// reported through [`FrameSink::report_transient`] and skipped.
    fn publish(&self, sample_buffer: &CMSampleBuffer) {
        let ivars = self.ivars();
        let sink = ivars.sink.sink();
        let host_timestamp = ivars.epoch.elapsed();

        // SAFETY: `sample_buffer` is the live buffer AVFoundation passed to
        // this callback; reading its presentation timestamp neither mutates nor
        // retains it.
        let presentation = unsafe { sample_buffer.presentation_time_stamp() };
        let (timestamp, timestamp_source) = match cmtime_to_duration(
            presentation.value,
            presentation.timescale,
            presentation.flags.0,
        ) {
            Some(device_time) => (device_time, TimestampSource::DeviceMonotonic),
            // No usable device clock: report host arrival rather than
            // inventing a device timeline. `frame.rs` documents that the
            // two stamps are the same value for a host-arrival source.
            None => (host_timestamp, TimestampSource::HostArrival),
        };

        // SAFETY: as above — `image_buffer` reads the sample buffer's image
        // buffer and returns a retained handle, so the returned buffer stays
        // alive for as long as `image` is held.
        let Some(image) = (unsafe { sample_buffer.image_buffer() }) else {
            sink.report_transient(&CaptureError::platform(
                "sample buffer carried no image buffer",
            ));
            return;
        };

        let payload = match pixel::copy_video_frame(&image, ivars.format, &ivars.device) {
            Ok(video) => FramePayload::Raw(video),
            Err(error) => {
                sink.report_transient(&error);
                return;
            }
        };
        // The pixel buffer is released here, before the sink's mutex is taken.
        drop(image);

        // Only now, with a frame that will actually be published, does the
        // counter move.
        let sequence = ivars.sequence.fetch_add(1, Ordering::Relaxed);
        sink.deliver(CaptureFrame {
            format: ivars.format,
            payload,
            timestamp,
            host_timestamp,
            timestamp_source,
            sequence,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use objc2::ClassType;
    use oximedia_core::PixelFormat;

    use super::*;
    use crate::backend::{FrameMessage, StopSignal};
    use crate::config::DropPolicy;
    use crate::device::CaptureEncoding;
    use crate::session::StatsInner;

    fn format() -> CaptureFormat {
        CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 64, 32, 30, 1)
    }

    fn sink() -> (
        FrameSink,
        crossbeam_channel::Receiver<FrameMessage>,
        Arc<StatsInner>,
    ) {
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let stats = Arc::new(StatsInner::default());
        let sink = FrameSink::new(
            sender,
            receiver.clone(),
            DropPolicy::DropOldest,
            StopSignal::new(),
            Arc::clone(&stats),
        );
        (sink, receiver, stats)
    }

    /// The soundness of `unsafe impl Send + Sync for SinkHandle` rests entirely
    /// on `FrameSink` being thread-safe. Asserting it here turns a comment into
    /// a compile error: adding, say, a `Cell` to `FrameSink` would otherwise
    /// make both impls unsound in silence.
    #[test]
    fn the_sink_handle_assumption_is_checked_by_the_compiler() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FrameSink>();
    }

    #[test]
    fn the_delegate_class_registers_and_conforms() {
        let (sink, _receiver, _stats) = sink();
        // SAFETY: `sink` outlives `delegate`, which is dropped at the end of
        // this scope and never handed to AVFoundation.
        let delegate = unsafe { SampleDelegate::new(&sink, format(), "test".into()) };
        assert!(delegate.isKindOfClass(NSObject::class()));
        let _protocol = delegate.as_protocol();
    }

    #[test]
    fn the_sink_handle_borrows_the_real_sink() {
        let (sink, _receiver, stats) = sink();
        // SAFETY: `sink` outlives `delegate`.
        let delegate = unsafe { SampleDelegate::new(&sink, format(), "test".into()) };
        delegate.ivars().sink.sink().report_device_dropped(3);
        assert_eq!(stats.snapshot().device_dropped, 3);
    }

    #[test]
    fn a_fresh_delegate_starts_at_sequence_zero() {
        let (sink, _receiver, _stats) = sink();
        // SAFETY: `sink` outlives `delegate`.
        let delegate = unsafe { SampleDelegate::new(&sink, format(), "test".into()) };
        assert_eq!(delegate.ivars().sequence.load(Ordering::Relaxed), 0);
        assert_eq!(delegate.ivars().format, format());
        assert_eq!(delegate.ivars().device, "test");
        assert!(delegate.ivars().epoch.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn dropped_sample_buffers_are_attributed_to_the_device() {
        let (sink, _receiver, stats) = sink();
        // SAFETY: `sink` outlives `delegate`.
        let delegate = unsafe { SampleDelegate::new(&sink, format(), "test".into()) };
        for _ in 0..4 {
            delegate.ivars().sink.sink().report_device_dropped(1);
        }
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.device_dropped, 4);
        assert_eq!(snapshot.dropped, 0, "not this crate's queue");
        assert_eq!(snapshot.errors, 0, "a dropped frame is not an error");
    }
}
