//! AVFoundation bindings: authorization, discovery and session lifecycle.
//!
//! Everything here runs Objective-C. The division of labour with
//! [`super`] is deliberate: this file makes framework calls and translates
//! their outcomes into [`CaptureError`]s, while every pure decision it depends
//! on — which four-character codes are understood, how a `CMTime` becomes a
//! [`std::time::Duration`], what rational frame rate a frame duration encodes —
//! lives in the parent module, where it is unit-tested with no camera present.
//!
//! # Permission
//!
//! macOS gates camera access behind TCC. The status is checked *before* any
//! device object is created, because `-[AVCaptureDeviceInput initWithDevice:]`
//! will happily succeed while access is undetermined and the device will then
//! "vend black video frames" (Apple's words) until the user answers the dialog.
//! Publishing those would be fabricating frames, so:
//!
//! * `Denied` / `Restricted` — [`CaptureError::PermissionDenied`], immediately;
//! * `NotDetermined` — enumeration proceeds (listing devices needs no
//!   permission), but opening one first requests access and waits, bounded, for
//!   the answer;
//! * `Authorized` — proceed.

#![allow(unsafe_code)]

use std::time::Instant;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceFormat,
    AVCaptureDeviceInput, AVCaptureDevicePosition, AVCaptureDeviceType,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeDeskViewCamera,
    AVCaptureDeviceTypeExternal, AVCaptureSession, AVCaptureSessionPresetInputPriority,
    AVCaptureVideoDataOutput, AVMediaType, AVMediaTypeVideo,
};
use objc2_core_media::{CMTime, CMTimeFlags, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::kCVPixelBufferPixelFormatTypeKey;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use crate::backend::{FrameSink, StopSignal};
use crate::device::{BackendKind, CaptureDevice, CaptureFormat, DeviceSelector};
use crate::error::CaptureError;

use super::delegate::SampleDelegate;
use super::{
    encoding_for_fourcc, fourcc_name, fps_from_frame_duration, frame_duration_for_fps, AvfRunner,
    AUTHORIZATION_TIMEOUT, CALLBACK_QUEUE_LABEL, PARK_INTERVAL, VIDEO_CAPTURE_SUBJECT,
};

// ── Framework constants ──────────────────────────────────────────────────────

/// `AVMediaTypeVideo`.
///
/// Returns an error rather than unwrapping: the binding types the constant as
/// `Option<&AVMediaType>` because a framework that failed to load leaves it
/// null, and this crate does not panic on a hostile environment.
fn video_media_type() -> Result<&'static AVMediaType, CaptureError> {
    // SAFETY: reading an immutable `extern "C"` string constant exported by
    // AVFoundation. The framework is linked into this binary by objc2's link
    // attribute, so the symbol is resolved before any Rust code runs.
    unsafe { AVMediaTypeVideo }.ok_or_else(|| {
        CaptureError::platform("AVMediaTypeVideo is unavailable; AVFoundation did not load")
    })
}

/// The device types this backend discovers.
///
/// Built-in wide-angle covers every Mac's own camera; external covers USB/UVC;
/// desk view is the overhead-crop virtual device of a Continuity Camera. The
/// Continuity Camera type itself is deliberately omitted: reporting it requires
/// the `NSCameraUseContinuityCameraDeviceType` Info.plist key, and without that
/// key macOS presents the same device as a built-in wide-angle camera — so it
/// is discovered anyway, and asking for the type would only add a mode that a
/// plain library consumer cannot enable.
fn discovery_device_types() -> Retained<NSArray<AVCaptureDeviceType>> {
    // SAFETY: three immutable `extern "C"` string constants exported by
    // AVFoundation, read to build an array the discovery session copies.
    let types = unsafe {
        [
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeDeskViewCamera,
        ]
    };
    NSArray::from_slice(&types)
}

/// `kCVPixelBufferPixelFormatTypeKey`, as the `NSString` an `NSDictionary` key
/// must be.
fn pixel_format_key() -> &'static NSString {
    // SAFETY: reading an immutable `extern "C"` constant exported by
    // CoreVideo, then re-typing it from `CFString` to `NSString`. `CFString`
    // and `NSString` are toll-free bridged: they are the same object at
    // runtime, and Core Foundation guarantees a `CFStringRef` may be used
    // wherever an `NSString *` is expected. Both are opaque, so the cast
    // changes only the Rust-side type used to message the object.
    unsafe {
        let key: *const _ = kCVPixelBufferPixelFormatTypeKey;
        &*key.cast::<NSString>()
    }
}

// ── Authorization ────────────────────────────────────────────────────────────

/// The process's current camera authorization.
fn authorization_status(media_type: &AVMediaType) -> AVAuthorizationStatus {
    // SAFETY: a class method with no preconditions beyond the media type being
    // `AVMediaTypeVideo` or `AVMediaTypeAudio`; the only caller passes the
    // former.
    unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) }
}

/// Whether the current status already rules capture out.
fn refusal(status: AVAuthorizationStatus, device: &str) -> Option<CaptureError> {
    if status == AVAuthorizationStatus::Denied || status == AVAuthorizationStatus::Restricted {
        Some(CaptureError::PermissionDenied {
            device: device.to_owned(),
        })
    } else {
        None
    }
}

/// Ensure the process may open a camera, prompting if it has never been asked.
///
/// Blocks for at most [`AUTHORIZATION_TIMEOUT`], and gives up early when `stop`
/// is raised, so a session that is dropped while the dialog is on screen still
/// joins its capture thread promptly.
fn require_authorization(
    media_type: &AVMediaType,
    device: &str,
    stop: &StopSignal,
) -> Result<(), CaptureError> {
    let status = authorization_status(media_type);
    if let Some(error) = refusal(status, device) {
        return Err(error);
    }
    if status == AVAuthorizationStatus::Authorized {
        return Ok(());
    }

    tracing::info!(device, "requesting camera access");
    let (sender, receiver) = crossbeam_channel::bounded::<bool>(1);
    let handler = RcBlock::new(move |granted: Bool| {
        // The handler runs once, on an arbitrary queue. `try_send` cannot
        // block and cannot panic, so nothing here can unwind into Objective-C.
        let _ = sender.try_send(granted.as_bool());
    });
    // SAFETY: `media_type` is `AVMediaTypeVideo`, and `handler` is a live block
    // whose `RcBlock` is kept alive across this call; AVFoundation copies it.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler) };

    let deadline = Instant::now() + AUTHORIZATION_TIMEOUT;
    loop {
        if stop.is_stopped() {
            return Err(CaptureError::PermissionDenied {
                device: device.to_owned(),
            });
        }
        match receiver.recv_timeout(PARK_INTERVAL) {
            Ok(true) => return Ok(()),
            Ok(false) => {
                return Err(CaptureError::PermissionDenied {
                    device: device.to_owned(),
                })
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(CaptureError::platform(
                    "camera access request ended without an answer",
                ))
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    tracing::warn!(
                        device,
                        timeout = ?AUTHORIZATION_TIMEOUT,
                        "camera access was never granted"
                    );
                    return Err(CaptureError::PermissionDenied {
                        device: device.to_owned(),
                    });
                }
            }
        }
    }
}

// ── Enumeration ──────────────────────────────────────────────────────────────

/// List every video capture device AVFoundation reports.
pub(crate) fn enumerate_devices() -> Result<Vec<CaptureDevice>, CaptureError> {
    let media_type = video_media_type()?;
    // `NotDetermined` is fine here: listing devices, their names and their
    // formats needs no permission, and prompting for one during a plain
    // `enumerate()` would be a surprise.
    if let Some(error) = refusal(authorization_status(media_type), VIDEO_CAPTURE_SUBJECT) {
        return Err(error);
    }
    Ok(discover_devices(media_type))
}

/// Run the discovery session and describe what it found.
///
/// Split out from [`enumerate_devices`] so that the framework half can be
/// exercised on a machine whose TCC answer is `Denied` — the refusal check
/// short-circuits before this point, and without the split the whole of
/// discovery, the four-character-code mapping and the frame-rate extraction
/// would have no coverage on such a machine.
fn discover_devices(media_type: &AVMediaType) -> Vec<CaptureDevice> {
    let types = discovery_device_types();
    // SAFETY: the device-type array and media type are both valid and outlive
    // the call, which copies what it needs.
    let session = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            Some(media_type),
            AVCaptureDevicePosition::Unspecified,
        )
    };
    // SAFETY: a property read on a live discovery session.
    let devices = unsafe { session.devices() };

    let mut listed = Vec::with_capacity(devices.len());
    for device in devices.iter() {
        // SAFETY: property reads on a live `AVCaptureDevice`.
        let (id, name) = unsafe { (device.uniqueID(), device.localizedName()) };
        let id = id.to_string();
        let formats = describe_formats(&device, &id);
        if formats.is_empty() {
            tracing::debug!(device = %id, "device advertises no format this backend understands");
        }
        listed.push(CaptureDevice {
            id,
            name: name.to_string(),
            formats,
            backend: BackendKind::AvFoundation,
        });
    }
    listed
}

/// Translate one device's `formats` array into [`CaptureFormat`]s.
///
/// Modes whose media sub-type is not in [`encoding_for_fourcc`]'s table are
/// skipped with a `debug!` line, never guessed at.
fn describe_formats(device: &AVCaptureDevice, device_id: &str) -> Vec<CaptureFormat> {
    // SAFETY: a property read on a live `AVCaptureDevice`.
    let device_formats = unsafe { device.formats() };
    let mut formats: Vec<CaptureFormat> = Vec::with_capacity(device_formats.len());

    for device_format in device_formats.iter() {
        let Some((encoding, width, height)) = describe_geometry(&device_format, device_id) else {
            continue;
        };
        let mut rates = Vec::new();
        // SAFETY: a property read on a live `AVCaptureDeviceFormat`.
        for range in unsafe { device_format.videoSupportedFrameRateRanges() }.iter() {
            // SAFETY: property reads on a live `AVFrameRateRange`. The minimum
            // frame *duration* is the maximum frame *rate*, and vice versa.
            let (fastest, slowest) =
                unsafe { (range.minFrameDuration(), range.maxFrameDuration()) };
            for (value, timescale) in [
                (fastest.value, fastest.timescale),
                (slowest.value, slowest.timescale),
            ] {
                if let Some(rate) = fps_from_frame_duration(value, timescale) {
                    if !rates.contains(&rate) {
                        rates.push(rate);
                    }
                }
            }
        }
        if rates.is_empty() {
            // The mode exists but reports no usable rate. `CaptureFormat`
            // documents `fps_den == 0` as "unknown / variable", which is the
            // honest record; inventing 30 fps here would be a guess.
            tracing::debug!(
                device = %device_id,
                %encoding,
                width,
                height,
                "format reports no usable frame-rate range"
            );
            rates.push((0, 0));
        }
        for (fps_num, fps_den) in rates {
            let candidate = CaptureFormat::new(encoding, width, height, fps_num, fps_den);
            if !formats.contains(&candidate) {
                formats.push(candidate);
            }
        }
    }
    formats
}

/// Encoding and pixel dimensions of one `AVCaptureDeviceFormat`.
///
/// `None` when the media sub-type is unknown to this backend or the dimensions
/// are not positive.
fn describe_geometry(
    device_format: &AVCaptureDeviceFormat,
    device_id: &str,
) -> Option<(crate::device::CaptureEncoding, u32, u32)> {
    // SAFETY: a property read on a live `AVCaptureDeviceFormat`; the returned
    // description is retained for the duration of this function.
    let description = unsafe { device_format.formatDescription() };
    // SAFETY: both are pure accessors on a live `CMFormatDescription`. A video
    // format description is a `CMVideoFormatDescription`, which is the same
    // type, so `GetDimensions` is applicable.
    let (subtype, dimensions) = unsafe {
        (
            description.media_sub_type(),
            CMVideoFormatDescriptionGetDimensions(&description),
        )
    };
    let Some(encoding) = encoding_for_fourcc(subtype) else {
        tracing::debug!(
            device = %device_id,
            fourcc = %fourcc_name(subtype),
            "skipping format with an unsupported media sub-type"
        );
        return None;
    };
    let width = u32::try_from(dimensions.width).ok()?;
    let height = u32::try_from(dimensions.height).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((encoding, width, height))
}

// ── Session teardown ─────────────────────────────────────────────────────────

/// Owns every Objective-C object of a running capture and tears them down in
/// the one order that makes the delegate's sink pointer sound.
///
/// See [`super::delegate`] for why each step is needed. The guard is dropped on
/// the capture thread, which is the thread that created everything it holds.
struct SessionGuard {
    session: Retained<AVCaptureSession>,
    output: Retained<AVCaptureVideoDataOutput>,
    queue: DispatchRetained<DispatchQueue>,
    /// Held so the delegate outlives the session, and dropped last.
    _delegate: Retained<SampleDelegate>,
    stop: StopSignal,
    started: bool,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        // 1. Anything parked inside a blocking send must give up, or
        //    `-stopRunning` would wait on it and the join would never return.
        self.stop.stop();
        if self.started {
            // SAFETY: the session is live and not mid-configuration —
            // `commitConfiguration` always runs before `started` is set.
            unsafe { self.session.stopRunning() };
        }
        // 2. No further callback can be scheduled.
        // SAFETY: clearing the delegate is explicitly supported, and passing
        // `None` for the queue is required when the delegate is `None`.
        unsafe { self.output.setSampleBufferDelegate_queue(None, None) };
        // 3. The queue is serial, so this empty block cannot start until any
        //    in-flight callback body has returned.
        self.queue.exec_sync(|| {});
    }
}

// ── Capture ──────────────────────────────────────────────────────────────────

/// Open the device, run the session, and stop when told to.
///
/// Every Objective-C object this backend uses is created here and dropped
/// before the function returns — see the [module docs](super) on why that
/// cannot happen in `open()`.
pub(crate) fn run_capture(
    runner: &AvfRunner,
    sink: &FrameSink,
    stop: &StopSignal,
) -> Result<(), CaptureError> {
    let format = runner.format();
    let device_id = runner.device_id().to_owned();

    if format.encoding.is_compressed() {
        // The device advertises the mode and `enumerate()` reports it
        // faithfully, but `AVCaptureVideoDataOutput` is configured through
        // `kCVPixelBufferPixelFormatTypeKey` and delivers `CVPixelBuffer`s.
        // Claiming to deliver a JPEG bitstream and then handing over decoded
        // samples would be worse than refusing.
        return Err(CaptureError::format_rejected(
            device_id,
            format,
            "AVCaptureVideoDataOutput delivers uncompressed CVPixelBuffers; \
             this backend has no compressed delivery path",
        ));
    }

    let media_type = video_media_type()?;
    require_authorization(media_type, &device_id, stop)?;

    let unique_id = NSString::from_str(&device_id);
    // SAFETY: a class method taking a live `NSString`; it returns `None` when
    // no device carries that unique id.
    let device = unsafe { AVCaptureDevice::deviceWithUniqueID(&unique_id) }
        .ok_or_else(|| CaptureError::DeviceNotFound(DeviceSelector::Id(device_id.clone())))?;

    let (device_format, fourcc) = select_device_format(&device, format, &device_id)?;
    tracing::debug!(
        device = %device_id,
        fourcc = %fourcc_name(fourcc),
        negotiated = %format,
        "configuring AVFoundation capture"
    );

    // SAFETY: `-[AVCaptureDeviceInput deviceInputWithDevice:error:]` on a live
    // device. It fails when the device is in use or access is refused.
    let input =
        unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(|error| {
            CaptureError::platform(format!(
                "AVCaptureDeviceInput for {device_id:?} failed: {error}"
            ))
        })?;

    // SAFETY: plain designated constructors with no preconditions.
    let (session, output) = unsafe { (AVCaptureSession::new(), AVCaptureVideoDataOutput::new()) };
    let queue = DispatchQueue::new(CALLBACK_QUEUE_LABEL, DispatchQueueAttr::SERIAL);
    // SAFETY: `sink` is borrowed for the whole of this function, and the
    // `SessionGuard` built below guarantees that no callback body outlives the
    // guard's `Drop`, which runs before this function returns.
    let delegate = unsafe { SampleDelegate::new(sink, format, device_id.clone()) };

    // Let AVFoundation discard frames the delegate queue could not keep up
    // with; they are reported through `-captureOutput:didDropSampleBuffer:` and
    // counted as device-side losses. The *pixel format* is set later, in
    // `attach`, because the set of formats an output will accept is only known
    // once it is connected to an input with a chosen `activeFormat`.
    // SAFETY: a plain property write on a live output.
    unsafe { output.setAlwaysDiscardsLateVideoFrames(true) };
    // SAFETY: the delegate and the queue both outlive the guard, which clears
    // this association before dropping either.
    unsafe { output.setSampleBufferDelegate_queue(Some(delegate.as_protocol()), Some(&queue)) };

    let mut guard = SessionGuard {
        session,
        output,
        queue,
        _delegate: delegate,
        stop: stop.clone(),
        started: false,
    };

    // `beginConfiguration` must be matched by `commitConfiguration` before
    // `-startRunning` or `-stopRunning`, or AVFoundation raises
    // `NSGenericException`. There is no `?` between the two for that reason:
    // the result is captured and only propagated after the commit.
    // SAFETY: the session is live and no configuration block is open.
    unsafe { guard.session.beginConfiguration() };
    let chosen = Chosen {
        device: &device,
        device_format: &device_format,
        format,
        device_id: &device_id,
    };
    let configured = attach(&guard, &input, &chosen);
    // SAFETY: matching commit for the begin above.
    unsafe { guard.session.commitConfiguration() };
    configured?;

    configure_pixel_format(&guard.output, fourcc, format, &device_id)?;

    // SAFETY: configuration is committed, so starting is legal here.
    unsafe { guard.session.startRunning() };
    guard.started = true;
    // SAFETY: a property read on a live session.
    if !unsafe { guard.session.isRunning() } {
        return Err(CaptureError::format_rejected(
            device_id,
            format,
            "AVCaptureSession refused to start",
        ));
    }
    tracing::info!(
        device = %device_id,
        name = %runner.device_name(),
        negotiated = %format,
        "AVFoundation capture running"
    );

    park_until_stopped(&guard, stop, &device_id)
}

/// Point the output at the negotiated pixel format.
///
/// Runs *after* `commitConfiguration` and before `-startRunning`.
/// `videoSettings` is a plain property, so there is nothing to gain from
/// batching it into the configuration block, and
/// `-availableVideoCVPixelFormatTypes` is derived from the device's
/// `activeFormat` — which is only definitely in effect once the configuration
/// has been committed. No frame can arrive before `-startRunning`, so the
/// output is never live with the wrong setting.
///
/// An **empty** list is treated as "the output did not say", not as "the output
/// offers nothing": the property is documented as a hint, and refusing to open
/// on the strength of an empty hint would break capture outright on any macOS
/// release that populates it later than expected. A non-empty list that omits
/// the code is a real answer and is rejected, because the alternative is
/// handing the caller a layout it was not promised.
fn configure_pixel_format(
    output: &AVCaptureVideoDataOutput,
    fourcc: u32,
    format: CaptureFormat,
    device_id: &str,
) -> Result<(), CaptureError> {
    // SAFETY: a property read on a live output that is already attached to a
    // configured session; the returned array is retained for this function.
    let available = unsafe { output.availableVideoCVPixelFormatTypes() };
    let mut offered = Vec::with_capacity(available.len());
    for value in available.iter() {
        offered.push(value.as_u32());
    }
    if offered.is_empty() {
        tracing::debug!(
            device = %device_id,
            fourcc = %fourcc_name(fourcc),
            "AVCaptureVideoDataOutput reported no available pixel formats; requesting anyway"
        );
    } else if !offered.contains(&fourcc) {
        return Err(CaptureError::format_rejected(
            device_id,
            format,
            format!(
                "AVCaptureVideoDataOutput does not offer {}; it offers [{}]",
                fourcc_name(fourcc),
                offered
                    .iter()
                    .map(|&code| fourcc_name(code))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let value = NSNumber::numberWithUnsignedInt(fourcc);
    let settings: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&[pixel_format_key()], &[value.as_ref() as &AnyObject]);
    // SAFETY: the dictionary has the key and value types `-setVideoSettings:`
    // documents (`kCVPixelBufferPixelFormatTypeKey` -> `NSNumber`), the code is
    // one the output just said it offers, and the dictionary is copied by the
    // setter.
    unsafe { output.setVideoSettings(Some(&settings)) };
    Ok(())
}

/// The mode `attach` is about to pin the device to.
struct Chosen<'a> {
    /// The device being configured.
    device: &'a AVCaptureDevice,
    /// The device's own format object that produced [`Self::format`].
    device_format: &'a AVCaptureDeviceFormat,
    /// What `negotiate()` settled on.
    format: CaptureFormat,
    /// `AVCaptureDevice.uniqueID`, for errors.
    device_id: &'a str,
}

/// Wire the input and output into the session and pin the device's mode.
///
/// Called between `beginConfiguration` and `commitConfiguration`.
fn attach(
    guard: &SessionGuard,
    input: &AVCaptureDeviceInput,
    chosen: &Chosen<'_>,
) -> Result<(), CaptureError> {
    let Chosen {
        device,
        device_format,
        format,
        device_id,
    } = *chosen;
    // SAFETY: `canAddInput:`/`canAddOutput:` are the guarded form; calling
    // `addInput:` without them raises `NSInvalidArgumentException`.
    unsafe {
        if !guard.session.canAddInput(input) {
            return Err(CaptureError::format_rejected(
                device_id,
                format,
                "AVCaptureSession refused the device input",
            ));
        }
        guard.session.addInput(input);
        if !guard.session.canAddOutput(&guard.output) {
            return Err(CaptureError::format_rejected(
                device_id,
                format,
                "AVCaptureSession refused the video data output",
            ));
        }
        guard.session.addOutput(&guard.output);
    }

    // With any other preset the session owns the device's `activeFormat` and
    // would override the negotiated mode. Setting `activeFormat` switches the
    // preset to input priority anyway; doing it explicitly makes the intent
    // visible and fails loudly if the session disagrees.
    // SAFETY: reading an immutable framework string constant, then a guarded
    // property write.
    unsafe {
        if guard
            .session
            .canSetSessionPreset(AVCaptureSessionPresetInputPriority)
        {
            guard
                .session
                .setSessionPreset(AVCaptureSessionPresetInputPriority);
        }
    }

    apply_active_format(device, device_format, format, device_id)
}

/// Lock the device and pin it to the negotiated format and frame rate.
fn apply_active_format(
    device: &AVCaptureDevice,
    device_format: &AVCaptureDeviceFormat,
    format: CaptureFormat,
    device_id: &str,
) -> Result<(), CaptureError> {
    // SAFETY: `-lockForConfiguration:` on a live device; it reports an
    // `NSError` when another process holds the device.
    unsafe { device.lockForConfiguration() }.map_err(|error| {
        CaptureError::format_rejected(device_id, format, format!("lockForConfiguration: {error}"))
    })?;

    // `-setActiveVideoMinFrameDuration:` raises `NSInvalidArgumentException`
    // for a duration the *active* format does not support. `select_device_format`
    // already guarantees the rate is one of this format's own endpoints, but the
    // precondition is re-checked here rather than carried across a function
    // boundary as an assumption — an exception raised inside a `beginConfiguration`
    // block would leave the session wedged, and the check costs one pass over a
    // handful of ranges.
    let duration = offers_rate(device_format, format)
        .then(|| frame_duration_for_fps(format.fps_num, format.fps_den))
        .flatten();
    if duration.is_none() {
        tracing::debug!(
            device = %device_id,
            negotiated = %format,
            "leaving the frame rate to the device's default for this format"
        );
    }

    // No `?` between the lock and the unlock: an early return here would leave
    // the device locked for the lifetime of the process.
    // SAFETY: `device_format` came from this device's own `formats` array, so
    // `-setActiveFormat:` cannot raise, and `duration` is `Some` only when that
    // same format reports the rate as one of its supported endpoints — the
    // precondition `-setActiveVideoMinFrameDuration:` documents.
    unsafe {
        device.setActiveFormat(device_format);
        if let Some((value, timescale)) = duration {
            device.setActiveVideoMinFrameDuration(CMTime {
                value,
                timescale,
                flags: CMTimeFlags::Valid,
                epoch: 0,
            });
        }
        device.unlockForConfiguration();
    }
    Ok(())
}

/// Find the `AVCaptureDeviceFormat` that produced `wanted`, and its four-char
/// code.
///
/// Matching is on encoding, dimensions and frame rate, and all three must hold.
/// Two device formats can map to the same [`CaptureFormat`] — `420v` and `420f`
/// are both NV12 — and the first match in enumeration order is taken, which is
/// the same tie-break [`crate::negotiate()`] applies, so the mode that was
/// ranked is the mode that gets configured.
///
/// A mode whose size and encoding still exist but whose frame rate no longer
/// does is [`CaptureError::FormatRejected`], not a silent downgrade. Accepting
/// it would leave every delivered frame — and
/// [`CaptureSession::negotiated_format`](crate::CaptureSession::negotiated_format),
/// documented as "the format the device was actually configured for" — claiming
/// a rate nothing backs.
fn select_device_format(
    device: &AVCaptureDevice,
    wanted: CaptureFormat,
    device_id: &str,
) -> Result<(Retained<AVCaptureDeviceFormat>, u32), CaptureError> {
    // SAFETY: a property read on a live `AVCaptureDevice`.
    let device_formats = unsafe { device.formats() };
    let mut geometry_only = false;

    for device_format in device_formats.iter() {
        // SAFETY: a property read on a live `AVCaptureDeviceFormat`.
        let description = unsafe { device_format.formatDescription() };
        // SAFETY: pure accessors on a live `CMFormatDescription`.
        let (subtype, dimensions) = unsafe {
            (
                description.media_sub_type(),
                CMVideoFormatDescriptionGetDimensions(&description),
            )
        };
        if encoding_for_fourcc(subtype) != Some(wanted.encoding)
            || u32::try_from(dimensions.width).ok() != Some(wanted.width)
            || u32::try_from(dimensions.height).ok() != Some(wanted.height)
        {
            continue;
        }
        geometry_only = true;
        // `fps_den == 0` is `CaptureFormat`'s "unknown / variable rate", which
        // `describe_formats` emits for a mode whose ranges it could not read.
        // There is nothing to match against, so the device keeps its default.
        if wanted.fps_den == 0 || offers_rate(&device_format, wanted) {
            return Ok((device_format, subtype));
        }
    }

    if geometry_only {
        return Err(CaptureError::format_rejected(
            device_id,
            wanted,
            "the device still offers this size and encoding, but no longer at this frame rate",
        ));
    }

    Err(CaptureError::format_rejected(
        device_id,
        wanted,
        "the device no longer advertises this mode",
    ))
}

/// Whether `device_format` supports `wanted`'s exact rational frame rate.
fn offers_rate(device_format: &AVCaptureDeviceFormat, wanted: CaptureFormat) -> bool {
    // SAFETY: a property read on a live `AVCaptureDeviceFormat`.
    for range in unsafe { device_format.videoSupportedFrameRateRanges() }.iter() {
        // SAFETY: property reads on a live `AVFrameRateRange`.
        let (fastest, slowest) = unsafe { (range.minFrameDuration(), range.maxFrameDuration()) };
        for (value, timescale) in [
            (fastest.value, fastest.timescale),
            (slowest.value, slowest.timescale),
        ] {
            if fps_from_frame_duration(value, timescale) == Some((wanted.fps_num, wanted.fps_den)) {
                return true;
            }
        }
    }
    false
}

/// Park the capture thread until the stop flag is raised or the session dies.
///
/// AVFoundation pushes frames from its own queue, so this thread has nothing to
/// do but stay alive and answer [`crate::CaptureSession::stop`] quickly.
fn park_until_stopped(
    guard: &SessionGuard,
    stop: &StopSignal,
    device_id: &str,
) -> Result<(), CaptureError> {
    while !stop.is_stopped() {
        std::thread::sleep(PARK_INTERVAL);
        // SAFETY: a property read on a live session.
        if !unsafe { guard.session.isRunning() } {
            // A device that was unplugged, or a runtime error AVFoundation
            // reported through its notification centre, stops the session
            // without telling this thread. Ending with an error is the honest
            // outcome; ending with `Ok(())` would look like a clean shutdown.
            return Err(CaptureError::platform(format!(
                "AVCaptureSession for {device_id:?} stopped unexpectedly"
            )));
        }
    }
    Ok(())
}

/// How long [`park_until_stopped`] can take to notice a raised flag.
///
/// Exposed for the test below so the shutdown-latency promise in the crate's
/// README is checked without a camera.
#[cfg(test)]
const MAX_STOP_LATENCY: std::time::Duration = PARK_INTERVAL;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_video_media_type_constant_resolves() {
        assert!(
            video_media_type().is_ok(),
            "AVFoundation is linked into the test binary"
        );
    }

    #[test]
    fn the_discovery_device_type_list_is_the_documented_three() {
        assert_eq!(discovery_device_types().len(), 3);
    }

    #[test]
    fn the_pixel_format_key_bridges_to_an_ns_string() {
        assert_eq!(pixel_format_key().to_string(), "PixelFormatType");
    }

    #[test]
    fn denial_and_restriction_are_permission_errors() {
        for status in [
            AVAuthorizationStatus::Denied,
            AVAuthorizationStatus::Restricted,
        ] {
            match refusal(status, VIDEO_CAPTURE_SUBJECT) {
                Some(CaptureError::PermissionDenied { device }) => {
                    assert_eq!(device, VIDEO_CAPTURE_SUBJECT);
                }
                other => panic!("expected PermissionDenied for {status:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn authorized_and_undetermined_do_not_refuse_enumeration() {
        // Listing devices works before the user has ever been asked; only
        // opening one needs an answer.
        assert!(refusal(AVAuthorizationStatus::Authorized, "x").is_none());
        assert!(refusal(AVAuthorizationStatus::NotDetermined, "x").is_none());
    }

    #[test]
    fn the_authorization_status_query_does_not_prompt() {
        // Reading the status is side-effect free — this is what makes
        // `enumerate()` safe to call from a test suite.
        let media_type = video_media_type().expect("AVFoundation is linked");
        let status = authorization_status(media_type);
        assert!(
            [
                AVAuthorizationStatus::NotDetermined,
                AVAuthorizationStatus::Restricted,
                AVAuthorizationStatus::Denied,
                AVAuthorizationStatus::Authorized,
            ]
            .contains(&status),
            "unexpected AVAuthorizationStatus {status:?}"
        );
    }

    #[test]
    fn a_raised_stop_flag_short_circuits_the_permission_wait() {
        // With the flag already up, the wait must not reach the 60 s timeout.
        let media_type = video_media_type().expect("AVFoundation is linked");
        if authorization_status(media_type) != AVAuthorizationStatus::NotDetermined {
            // Nothing to test on a machine that has already answered.
            return;
        }
        let stop = StopSignal::new();
        stop.stop();
        let started = Instant::now();
        let result = require_authorization(media_type, "device", &stop);
        assert!(matches!(result, Err(CaptureError::PermissionDenied { .. })));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_stop_poll_interval_keeps_shutdown_well_under_the_promised_budget() {
        assert!(MAX_STOP_LATENCY < Duration::from_millis(500));
    }

    /// Exercises the whole framework half of enumeration — discovery session,
    /// `uniqueID`/`localizedName`, the four-character-code mapping,
    /// `CMVideoFormatDescriptionGetDimensions` and the frame-rate-range
    /// extraction — against whatever hardware this machine has.
    ///
    /// It bypasses the permission check on purpose: TCC gates the media
    /// *stream*, not device metadata, so discovery works regardless, and going
    /// through `enumerate_devices` on a machine that has denied access would
    /// short-circuit and leave all of the above untested. It prompts for
    /// nothing.
    #[test]
    fn discovery_describes_real_devices_without_prompting() {
        let media_type = video_media_type().expect("AVFoundation is linked");
        let devices = discover_devices(media_type);
        for device in &devices {
            assert!(!device.id.is_empty(), "every device needs a stable id");
            assert_eq!(device.backend, BackendKind::AvFoundation);
            for format in &device.formats {
                assert!(format.width > 0, "{format}");
                assert!(format.height > 0, "{format}");
                // `describe_formats` only ever emits a rate it could read
                // exactly, or the documented "unknown" pair.
                assert!(
                    (format.fps_num == 0) == (format.fps_den == 0),
                    "half-known frame rate {}/{}",
                    format.fps_num,
                    format.fps_den
                );
                assert!(
                    matches!(
                        format.encoding,
                        crate::device::CaptureEncoding::Raw(_)
                            | crate::device::CaptureEncoding::Mjpeg
                    ),
                    "{format}"
                );
            }
            // Enumeration order is `negotiate()`'s final tie-break, so a
            // duplicate entry would make selection ambiguous.
            for (index, format) in device.formats.iter().enumerate() {
                assert!(
                    !device.formats[index + 1..].contains(format),
                    "duplicate format {format} on {}",
                    device.id
                );
            }
        }
    }

    #[test]
    fn enumeration_is_honest_about_what_it_found() {
        // Runs on this machine without prompting: either the platform lists
        // devices, or it refuses because permission was already denied. It must
        // never report a device without a stable id.
        match enumerate_devices() {
            Ok(devices) => {
                for device in devices.iter() {
                    assert!(!device.id.is_empty(), "{device}");
                    assert_eq!(device.backend, BackendKind::AvFoundation);
                    for format in &device.formats {
                        assert!(format.width > 0 && format.height > 0, "{format}");
                    }
                }
            }
            Err(CaptureError::PermissionDenied { device }) => {
                assert_eq!(device, VIDEO_CAPTURE_SUBJECT);
            }
            Err(other) => panic!("unexpected enumeration failure: {other:?}"),
        }
    }
}
