//! `IMFSourceReader`: format discovery and the capture loop.
//!
//! Everything that drives a source reader lives here — walking a device's
//! native media types during enumeration, pinning one of them for a session,
//! and pulling samples until the session ends. The *decisions* those calls feed
//! on are not here: how a packed `u64` attribute becomes a size or an exact
//! rational rate, how `MF_MT_DEFAULT_STRIDE` describes a buffer's rows, how a
//! contiguous buffer becomes planes, and which `HRESULT`s end a session all
//! live in [`super::super::mf_logic`], which is compiled and tested on every
//! host rather than only on Windows.
//!
//! # Native formats only
//!
//! The reader is created **without** `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING`.
//! With that attribute set, Media Foundation silently inserts a converter and
//! answers a `SetCurrentMediaType` for a format the camera cannot produce —
//! which would make [`CaptureSession::negotiated_format`](crate::CaptureSession::negotiated_format)
//! describe a conversion rather than the device. Leaving it off is what makes
//! "this is what the camera produced" true.
//!
//! The echo-check after `SetCurrentMediaType` is a *different* guarantee, and
//! worth being precise about: `GetCurrentMediaType` returns the type the reader
//! settled on, so it catches a reader that accepted the request and then chose
//! a different size, subtype or frame rate. It cannot detect an inserted
//! converter — the type would look identical. Only the absent attribute above
//! rules that out.
//!
//! # Blocking and shutdown
//!
//! `ReadSample` is called synchronously with no control flags, so it returns as
//! soon as the next frame is ready — one frame period at worst. The stop flag
//! is therefore checked at least once per frame, which is the contract
//! [`CaptureRunner`](crate::backend::CaptureRunner) documents, and
//! [`CaptureSession::stop`](crate::CaptureSession::stop) never waits longer
//! than that on the join.

#![allow(unsafe_code)]

use std::time::{Duration, Instant};

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaBuffer, IMFMediaSource, IMFMediaType, IMFSample, IMFSourceReader,
    MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFMediaType_Video,
    MF_E_ATTRIBUTENOTFOUND, MF_E_HW_MFT_FAILED_START_STREAMING, MF_E_HW_STREAM_NOT_CONNECTED,
    MF_E_INVALIDREQUEST, MF_E_NO_MORE_TYPES, MF_E_SHUTDOWN,
    MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED, MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
};

use super::super::mf_logic as logic;
use super::{
    com, encoding_for_subtype, subtype_for_encoding, subtype_name, MfRunner, ALL_STREAMS,
    FIRST_VIDEO_STREAM, MAX_NATIVE_TYPES, PARK_INTERVAL, READERF_CURRENTMEDIATYPECHANGED,
    READERF_ENDOFSTREAM, READERF_ERROR, READERF_STREAMTICK,
};
use crate::backend::{FrameSink, StopSignal};
use crate::device::CaptureFormat;
use crate::error::CaptureError;
use crate::frame::{CaptureFrame, FramePayload, TimestampSource};

// ── Hand-written HRESULT values, checked against the headers ─────────────────

// `mf_logic` names these `HRESULT`s as plain `i32`s so that its classifier can
// be compiled and tested on a host that has no `windows` crate. These
// assertions are the link back: they fail *at compile time* on the Windows
// target if a hand-written constant ever drifts from the value the crate's own
// generated bindings carry, so a `cargo check --target x86_64-pc-windows-msvc`
// from any host proves the table.
const _: () = assert!(logic::MF_E_INVALIDREQUEST == MF_E_INVALIDREQUEST.0);
const _: () =
    assert!(logic::MF_E_HW_MFT_FAILED_START_STREAMING == MF_E_HW_MFT_FAILED_START_STREAMING.0);
const _: () = assert!(logic::MF_E_HW_STREAM_NOT_CONNECTED == MF_E_HW_STREAM_NOT_CONNECTED.0);
const _: () = assert!(logic::MF_E_SHUTDOWN == MF_E_SHUTDOWN.0);
const _: () = assert!(
    logic::MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED == MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED.0
);
const _: () = assert!(
    logic::MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED == MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED.0
);

/// One tick of `llTimestamp`, which Media Foundation reports in 100 ns units.
const HNS_IN_NANOS: u64 = 100;

// ── Buffer lock ──────────────────────────────────────────────────────────────

/// A locked `IMFMediaBuffer`.
///
/// `Lock` and `Unlock` must be paired; a buffer left locked is never returned
/// to the source's pool, and the device starts dropping frames a few samples
/// later. Every failure path in the copy below is an early `return`, so pairing
/// them by hand would be one forgotten branch away from wedging capture.
struct BufferLock<'buffer> {
    buffer: &'buffer IMFMediaBuffer,
    data: *mut u8,
    length: usize,
}

impl<'buffer> BufferLock<'buffer> {
    /// Lock `buffer` and record where its valid bytes are.
    fn acquire(buffer: &'buffer IMFMediaBuffer) -> Result<Self, CaptureError> {
        let mut data: *mut u8 = core::ptr::null_mut();
        let mut length = 0_u32;
        // SAFETY: `buffer` is a live media buffer; both out-parameters point at
        // live locals. The maximum length is not needed — only the bytes the
        // producer marked valid may be read — so `None` is passed for it.
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }
            .map_err(|error| com::platform_error("IMFMediaBuffer::Lock", &error))?;
        if data.is_null() {
            // SAFETY: the lock succeeded, so it must be released even though
            // the pointer is unusable.
            let _ = unsafe { buffer.Unlock() };
            return Err(CaptureError::platform(
                "IMFMediaBuffer::Lock returned a null base address",
            ));
        }
        Ok(Self {
            buffer,
            data,
            length: length as usize,
        })
    }

    /// The buffer's valid bytes.
    fn bytes(&self) -> &[u8] {
        // SAFETY: `Lock` reported `length` valid bytes at `data` and the lock is
        // held for the whole of `'_`, so the slice cannot outlive it. The
        // pointer was checked non-null in `acquire`.
        unsafe { core::slice::from_raw_parts(self.data, self.length) }
    }
}

impl Drop for BufferLock<'_> {
    fn drop(&mut self) {
        // SAFETY: exactly one `Unlock` for the `Lock` in `acquire`, on a buffer
        // still borrowed for `'buffer`.
        if let Err(error) = unsafe { self.buffer.Unlock() } {
            tracing::warn!(error = %error, "IMFMediaBuffer::Unlock failed");
        }
    }
}

// ── Media types ──────────────────────────────────────────────────────────────

/// The media type a stream is currently set to, in this crate's terms.
#[derive(Debug, Clone, Copy)]
struct CurrentType {
    /// What the reader says it will deliver.
    format: CaptureFormat,
    /// Row layout of the delivered buffers; `None` for compressed encodings,
    /// which have no rows.
    order: Option<logic::RowOrder>,
}

/// Describe one media type, or skip it.
///
/// Returns `None` for a subtype this backend has no delivery path for. Such a
/// type is *skipped*, never guessed at: reporting it would advertise a capture
/// mode whose samples would then be mis-copied, which costs far more to
/// diagnose than a camera that appears to offer one mode fewer.
fn describe_media_type(media_type: &IMFMediaType, device_id: &str) -> Option<CaptureFormat> {
    // SAFETY: a live media type; `GetGUID` reads one attribute by key.
    let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.ok()?;
    let Some(encoding) = encoding_for_subtype(subtype) else {
        tracing::debug!(
            device = %device_id,
            subtype = %subtype_name(subtype),
            "skipping media type with an unsupported subtype"
        );
        return None;
    };

    // SAFETY: as above; the frame size is a packed 64-bit attribute.
    let packed_size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
    let (width, height) = logic::unpack_frame_size(packed_size)?;

    // A missing or nonsensical frame rate is `CaptureFormat`'s documented
    // "unknown / variable" pair, not an invented 30 fps.
    // SAFETY: as above.
    let rate = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }
        .ok()
        .and_then(logic::unpack_frame_rate);
    if rate.is_none() {
        tracing::debug!(
            device = %device_id,
            %encoding,
            width,
            height,
            "media type reports no usable frame rate"
        );
    }
    let (fps_num, fps_den) = rate.unwrap_or((0, 0));
    Some(CaptureFormat::new(
        encoding, width, height, fps_num, fps_den,
    ))
}

/// Read `MF_MT_DEFAULT_STRIDE`, treating "absent" as "packed".
///
/// The attribute is stored as a `UINT32` but holds a signed value, so the
/// round-trip through `i32` is the documented reading, not a cast that loses
/// information. An absent attribute is a normal answer — many camera types omit
/// it — and is reported as `None` rather than as a failure.
fn default_stride(media_type: &IMFMediaType) -> Option<i32> {
    // SAFETY: a live media type; `GetUINT32` reads one attribute by key.
    match unsafe { media_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) } {
        Ok(value) => Some(value as i32),
        Err(error) => {
            if error.code() != MF_E_ATTRIBUTENOTFOUND {
                tracing::debug!(error = %error, "MF_MT_DEFAULT_STRIDE could not be read");
            }
            None
        }
    }
}

/// Build the media type that asks a reader for `format`.
///
/// Only the four attributes that identify a mode are set. Media Foundation
/// matches a partial type against what the stream can actually produce, and the
/// echo-check in [`current_type`] is what confirms which mode it settled on.
fn build_media_type(format: CaptureFormat, device_id: &str) -> Result<IMFMediaType, CaptureError> {
    let Some(subtype) = subtype_for_encoding(format.encoding) else {
        return Err(CaptureError::format_rejected(
            device_id,
            format,
            "this backend has no Media Foundation subtype for that encoding",
        ));
    };

    // SAFETY: a plain factory call with no preconditions.
    let media_type = unsafe { MFCreateMediaType() }
        .map_err(|error| com::platform_error("MFCreateMediaType", &error))?;

    // SAFETY: a freshly created, live media type, and GUID keys and values that
    // outlive each call. Chained rather than `?`-ed one statement at a time so
    // that the whole sequence is one expression inside the `unsafe` block.
    let built = unsafe {
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .and_then(|()| media_type.SetGUID(&MF_MT_SUBTYPE, &subtype))
            .and_then(|()| {
                media_type.SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    logic::pack_frame_size(format.width, format.height),
                )
            })
            .and_then(|()| {
                // `fps_den == 0` is the documented "unknown / variable" rate.
                // Asking for it would be asking for a division by zero, so the
                // attribute is left off and the device keeps its own default.
                if format.fps_den == 0 {
                    Ok(())
                } else {
                    media_type.SetUINT64(
                        &MF_MT_FRAME_RATE,
                        logic::pack_frame_rate(format.fps_num, format.fps_den),
                    )
                }
            })
    };
    built.map_err(|error| {
        CaptureError::format_rejected(
            device_id,
            format,
            format!("building the requested media type failed: {error}"),
        )
    })?;
    Ok(media_type)
}

/// What the stream is currently set to deliver.
fn current_type(
    reader: &IMFSourceReader,
    device_id: &str,
    requested: CaptureFormat,
) -> Result<CurrentType, CaptureError> {
    // SAFETY: a live source reader and the video stream's index.
    let media_type =
        unsafe { reader.GetCurrentMediaType(FIRST_VIDEO_STREAM) }.map_err(|error| {
            CaptureError::format_rejected(
                device_id,
                requested,
                format!("GetCurrentMediaType failed: {error}"),
            )
        })?;

    let Some(format) = describe_media_type(&media_type, device_id) else {
        return Err(CaptureError::format_rejected(
            device_id,
            requested,
            "the stream is set to a media type this backend cannot describe",
        ));
    };

    let order = if format.encoding.is_compressed() {
        None
    } else {
        let Some(packed) = logic::packed_row_bytes(format) else {
            return Err(CaptureError::format_rejected(
                device_id,
                format,
                "no packed row length for this pixel layout",
            ));
        };
        let Some(order) = logic::row_order(default_stride(&media_type), packed) else {
            return Err(CaptureError::format_rejected(
                device_id,
                format,
                "the stream reports a zero MF_MT_DEFAULT_STRIDE",
            ));
        };
        Some(order)
    };
    Ok(CurrentType { format, order })
}

// ── Source reader ────────────────────────────────────────────────────────────

/// Build a source reader over `source`.
///
/// No attribute store is passed, which is how
/// `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING` is left off — see the module
/// documentation.
fn create_reader(source: &IMFMediaSource) -> Result<IMFSourceReader, CaptureError> {
    // SAFETY: a live media source and no attribute store.
    unsafe { MFCreateSourceReaderFromMediaSource(source, None) }
        .map_err(|error| com::platform_error("MFCreateSourceReaderFromMediaSource", &error))
}

/// Read every media type a device's video stream advertises.
///
/// Requires a live [`com::MfGuard`] on the calling thread; the caller
/// ([`super::MfBackend::enumerate`]) holds one for the whole enumeration.
///
/// Media Foundation has no way to list a device's modes without instantiating
/// its media source, so this briefly activates the camera and shuts it down
/// again. That is visible: on hardware with a privacy LED, enumeration can
/// flicker it. The alternative — reporting devices with no formats — would make
/// [`crate::negotiate()`] unable to choose anything.
///
/// A device that cannot be activated — because another process holds it, or
/// because the privacy setting refuses — yields an empty list with a `debug!`
/// line rather than failing the whole enumeration. The device is still
/// reported, with no formats, which is the honest record: it exists, and this
/// process could not ask it anything.
pub(crate) fn describe_formats(activate: &IMFActivate, device_id: &str) -> Vec<CaptureFormat> {
    let source = match com::activate_source(activate, device_id) {
        Ok(source) => source,
        Err(error) => {
            tracing::debug!(device = %device_id, %error, "device could not be activated to list its formats");
            return Vec::new();
        }
    };
    let reader = match create_reader(source.source()) {
        Ok(reader) => reader,
        Err(error) => {
            tracing::debug!(device = %device_id, %error, "no source reader for this device");
            return Vec::new();
        }
    };

    let mut formats: Vec<CaptureFormat> = Vec::new();
    for index in 0..MAX_NATIVE_TYPES {
        // SAFETY: a live source reader, the video stream's index, and a type
        // index that is bounded by the loop.
        match unsafe { reader.GetNativeMediaType(FIRST_VIDEO_STREAM, index) } {
            Ok(media_type) => {
                if let Some(format) = describe_media_type(&media_type, device_id) {
                    // Enumeration order is `negotiate()`'s final tie-break, so a
                    // duplicate would make selection ambiguous.
                    if !formats.contains(&format) {
                        formats.push(format);
                    }
                }
            }
            Err(error) if error.code() == MF_E_NO_MORE_TYPES => break,
            Err(error) => {
                tracing::debug!(
                    device = %device_id,
                    index,
                    %error,
                    "media type enumeration stopped early"
                );
                break;
            }
        }
    }
    if formats.is_empty() {
        tracing::debug!(device = %device_id, "device advertises no format this backend understands");
    }
    formats
}

// ── Capture ──────────────────────────────────────────────────────────────────

/// Open the device, pin the negotiated mode, and pump frames until told to stop.
///
/// Every COM object is created here, on the capture thread, and dropped before
/// this function returns — see the [module docs](super) for why none of it may
/// happen in `open()`. The platform guard is declared *first* so that Rust's
/// reverse-declaration drop order releases every interface pointer before
/// `MFShutdown` and `CoUninitialize` run.
pub(crate) fn run_capture(
    runner: &mut MfRunner,
    sink: &FrameSink,
    stop: &StopSignal,
) -> Result<(), CaptureError> {
    let guard = com::MfGuard::new()?;
    let device_id = runner.device_id().to_owned();
    let requested = runner.format();

    let activate = com::find_activate(&guard, &device_id)?;
    let source = com::activate_source(&activate, &device_id)?;
    let reader = create_reader(source.source())?;

    // A camera can expose more than one stream; reading only the video one
    // keeps the source from filling queues nothing drains.
    // SAFETY: a live source reader and the two documented stream constants.
    unsafe {
        reader
            .SetStreamSelection(ALL_STREAMS, false)
            .and_then(|()| reader.SetStreamSelection(FIRST_VIDEO_STREAM, true))
    }
    .map_err(|error| com::platform_error("IMFSourceReader::SetStreamSelection", &error))?;

    let media_type = build_media_type(requested, &device_id)?;
    // SAFETY: a live source reader, the video stream's index, no reserved
    // parameter, and a live media type built just above.
    unsafe { reader.SetCurrentMediaType(FIRST_VIDEO_STREAM, None, &media_type) }.map_err(
        |error| {
            CaptureError::format_rejected(
                &device_id,
                requested,
                format!("SetCurrentMediaType failed: {error}"),
            )
        },
    )?;

    // The echo check. It catches a reader that accepted the request and then
    // settled on a different size, subtype or frame rate; it cannot detect an
    // inserted converter — see the module docs on why none can be inserted.
    let mut current = current_type(&reader, &device_id, requested)?;
    if !logic::satisfies(requested, current.format) {
        return Err(CaptureError::format_rejected(
            &device_id,
            requested,
            format!("the reader settled on {} instead", current.format),
        ));
    }
    if current.format != requested {
        // The only way to get here is a request whose frame rate was the
        // documented "unknown / variable" pair, which the device has now filled
        // in. Recording it keeps every delivered frame's own description true.
        tracing::info!(
            device = %device_id,
            requested = %requested,
            settled = %current.format,
            "the device supplied the frame rate the request left open"
        );
        runner.set_format(current.format);
    }

    tracing::info!(
        device = %device_id,
        name = %runner.device_name(),
        negotiated = %current.format,
        apartment = ?guard.apartment(),
        "Media Foundation capture running"
    );

    // The loop's value is the tail expression on purpose. Rust drops locals in
    // reverse declaration order *after* it, which releases `media_type`, then
    // the reader, then the media source, then the activation object, and only
    // then runs `MFShutdown` and `CoUninitialize` in the guard — the one order
    // in which no COM object is released after the apartment it lives in has
    // been torn down. An explicit `drop(guard)` here would invert that.
    pump(&reader, runner, &mut current, sink, stop, &device_id)
}

/// Count one read that produced no frame, and park if the stream has stalled.
///
/// Called on every path through the loop that reaches `ReadSample` again
/// without publishing anything. The counter is reset by a delivered frame, so a
/// healthy stream never parks — see [`logic::should_park`].
fn park_if_stalled(frameless_reads: &mut u32) {
    *frameless_reads = frameless_reads.saturating_add(1);
    if logic::should_park(*frameless_reads) {
        std::thread::sleep(PARK_INTERVAL);
    }
}

/// The read loop, split out so `run_capture` reads as a setup sequence.
fn pump(
    reader: &IMFSourceReader,
    runner: &mut MfRunner,
    current: &mut CurrentType,
    sink: &FrameSink,
    stop: &StopSignal,
    device_id: &str,
) -> Result<(), CaptureError> {
    let epoch = Instant::now();
    let mut sequence = 0_u64;
    let mut warned_about_timestamps = false;
    // Reads since the last delivered frame. Several outcomes below return
    // immediately with nothing to publish, and a source that has stalled
    // without saying so can produce them forever; `logic::should_park` is what
    // keeps that from becoming a hot loop. See its documentation.
    let mut frameless_reads = 0_u32;

    while !stop.is_stopped() {
        let mut stream_index = 0_u32;
        let mut flags = 0_u32;
        let mut timestamp = 0_i64;
        let mut sample: Option<IMFSample> = None;
        // SAFETY: a live source reader and four out-parameters pointing at live
        // locals. The call is synchronous with no control flags, so it returns
        // once the next sample is ready.
        let read = unsafe {
            reader.ReadSample(
                FIRST_VIDEO_STREAM,
                0,
                Some(&mut stream_index),
                Some(&mut flags),
                Some(&mut timestamp),
                Some(&mut sample),
            )
        };
        let host_timestamp = epoch.elapsed();

        if let Err(error) = read {
            let reported = com::platform_error("IMFSourceReader::ReadSample", &error);
            match logic::classify_read_error(error.code().0) {
                logic::Fatality::Transient => {
                    sink.report_transient(&reported);
                    park_if_stalled(&mut frameless_reads);
                    continue;
                }
                logic::Fatality::Fatal => return Err(reported),
            }
        }

        if flags & READERF_ERROR != 0 {
            return Err(CaptureError::platform(format!(
                "the source reader for {device_id:?} reported an error flag"
            )));
        }
        if flags & READERF_ENDOFSTREAM != 0 {
            tracing::info!(device = %device_id, "capture stream ended");
            return Ok(());
        }
        if flags & READERF_CURRENTMEDIATYPECHANGED != 0 {
            // The sample delivered alongside this flag is already in the new
            // format, so the runner's idea of "current" has to move first.
            *current = current_type(reader, device_id, current.format)?;
            runner.set_format(current.format);
            tracing::info!(
                device = %device_id,
                format = %current.format,
                "the device changed the stream's media type mid-capture"
            );
        }
        if flags & READERF_STREAMTICK != 0 {
            // A gap the source is telling us about: frames were lost upstream
            // of this crate, which is not the same thing as this crate's queue
            // overflowing.
            sink.report_device_dropped(1);
            park_if_stalled(&mut frameless_reads);
            continue;
        }

        let Some(sample) = sample else {
            park_if_stalled(&mut frameless_reads);
            continue;
        };

        let (device_timestamp, timestamp_source) = if timestamp < 0 {
            if !warned_about_timestamps {
                warned_about_timestamps = true;
                tracing::warn!(
                    device = %device_id,
                    timestamp,
                    "the device reported a negative sample time; \
                     falling back to host arrival time for this session"
                );
            }
            (host_timestamp, TimestampSource::HostArrival)
        } else {
            match (timestamp.unsigned_abs()).checked_mul(HNS_IN_NANOS) {
                Some(nanos) => (
                    Duration::from_nanos(nanos),
                    TimestampSource::DeviceMonotonic,
                ),
                None => {
                    if !warned_about_timestamps {
                        warned_about_timestamps = true;
                        tracing::warn!(
                            device = %device_id,
                            timestamp,
                            "the device reported a sample time too large to represent; \
                             falling back to host arrival time for this session"
                        );
                    }
                    (host_timestamp, TimestampSource::HostArrival)
                }
            }
        };

        let payload = match copy_payload(&sample, *current, device_id) {
            Ok(payload) => payload,
            Err(error) => {
                sink.report_transient(&error);
                park_if_stalled(&mut frameless_reads);
                continue;
            }
        };

        // A frame is on its way out: the stream is not stalled.
        frameless_reads = 0;
        // Only a frame that is actually published moves the counter. Counting
        // skipped frames would open a gap that `FrameSink::deliver` attributes
        // to the *device*, turning this crate's own error into a fabricated
        // hardware fault.
        let published = sequence;
        sequence = sequence.saturating_add(1);
        sink.deliver(CaptureFrame {
            format: current.format,
            payload,
            timestamp: device_timestamp,
            host_timestamp,
            timestamp_source,
            sequence: published,
        });
    }
    Ok(())
}

/// Copy one sample's bytes out of Media Foundation's pool.
///
/// The sample references memory the capture pipeline owns and re-uses, so it is
/// read while locked and released immediately: holding samples starves the pool
/// and the device starts dropping frames.
fn copy_payload(
    sample: &IMFSample,
    current: CurrentType,
    device_id: &str,
) -> Result<FramePayload, CaptureError> {
    // SAFETY: a live sample. A camera sample normally has exactly one buffer,
    // in which case this returns it unchanged; otherwise the buffers are
    // concatenated into a fresh one, which is what the copy below expects.
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|error| com::platform_error("IMFSample::ConvertToContiguousBuffer", &error))?;
    let lock = BufferLock::acquire(&buffer)?;
    let bytes = lock.bytes();

    let payload = match current.order {
        // Compressed: the bitstream is handed over untouched. This crate does
        // not decode it — that would drag a JPEG decoder into every build that
        // only wanted to open a camera.
        None => {
            if bytes.is_empty() {
                return Err(CaptureError::format_rejected(
                    device_id,
                    current.format,
                    "the device delivered an empty compressed sample",
                ));
            }
            let mut data = Vec::with_capacity(bytes.len());
            data.extend_from_slice(bytes);
            FramePayload::Compressed(data)
        }
        Some(order) => FramePayload::Raw(logic::video_frame_from_contiguous(
            bytes,
            current.format,
            order,
            device_id,
        )?),
    };
    // The lock is released, and the buffer with it, before the caller takes the
    // sink's mutex.
    drop(lock);
    Ok(payload)
}
