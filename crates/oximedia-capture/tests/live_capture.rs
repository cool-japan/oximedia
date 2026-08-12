//! Live-device tests.
//!
//! These are the only tests in the crate that touch real hardware, and they are
//! doubly opt-in:
//!
//! * every one is `#[ignore]`, so `cargo test` never runs them;
//! * every one also requires `OXIMEDIA_CAPTURE_DEVICE` to name the device id to
//!   open, and *fails* rather than passes when it is unset.
//!
//! The second condition is the important one. A live test that quietly passes
//! on a machine with no camera is worse than no test: it turns a green CI run
//! into evidence of something that was never checked. Running them explicitly
//! and getting "the variable is not set" is an honest outcome; a silent pass is
//! not.
//!
//! ```bash
//! # macOS: the id is the AVCaptureDevice uniqueID reported by `enumerate()`
//! OXIMEDIA_CAPTURE_DEVICE=0x8020000005ac8514 \
//!     cargo test -p oximedia-capture --test live_capture -- --ignored
//! ```
//!
//! On macOS the first run raises the system camera-permission dialog, and the
//! camera indicator light comes on. That is why these do not run by default.

use std::time::{Duration, Instant};

use oximedia_capture::{
    enumerate, open, CaptureConfig, CaptureEncoding, CaptureFormat, CaptureFrame, CaptureSession,
    CaptureStream, DeviceSelector, FramePayload,
};
use oximedia_core::PixelFormat;

/// Environment variable naming the device to open.
const DEVICE_VAR: &str = "OXIMEDIA_CAPTURE_DEVICE";

/// How many frames a live capture must produce before the deadline.
const REQUIRED_FRAMES: usize = 10;

/// How long those frames may take. Ten frames is a third of a second at 30 fps;
/// five seconds leaves room for the device to warm up and auto-expose.
const FRAME_DEADLINE: Duration = Duration::from_secs(5);

/// The shutdown budget `CaptureSession::stop` promises.
const STOP_BUDGET: Duration = Duration::from_millis(500);

/// The device id under test, or a failure explaining how to supply one.
fn device_id() -> String {
    match std::env::var(DEVICE_VAR) {
        Ok(id) if !id.trim().is_empty() => id,
        _ => panic!(
            "{DEVICE_VAR} is not set. These tests need a real camera; set it to a device id \
             from `oximedia_capture::enumerate()` — e.g. {DEVICE_VAR}=0x8020000005ac8514"
        ),
    }
}

/// Open a session on the configured device.
fn open_configured() -> CaptureSession {
    let id = device_id();
    let config = CaptureConfig::default()
        .with_device(DeviceSelector::Id(id.clone()))
        .with_queue_depth(8);
    match open(config) {
        Ok(session) => session,
        Err(error) => panic!("could not open {id:?}: {error}"),
    }
}

/// Collect up to `count` frames, or as many as arrived before the deadline.
fn collect(stream: &mut CaptureStream, count: usize, deadline: Duration) -> Vec<CaptureFrame> {
    let started = Instant::now();
    let mut frames = Vec::with_capacity(count);
    while frames.len() < count && started.elapsed() < deadline {
        match stream.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) if stream.is_ended() => break,
            Ok(None) => continue,
            Err(error) => panic!("capture failed after {} frame(s): {error}", frames.len()),
        }
    }
    frames
}

/// Bytes one frame of `format` must carry.
///
/// `None` for MJPEG, whose length is a property of the image rather than of the
/// mode, and for odd dimensions, where `frame_buffer_size` truncates the chroma
/// plane (`(w/2)*2*(h/2)`) while the capture path rounds it up — no real camera
/// mode has an odd dimension, so the two conventions are never both exercised.
fn expected_payload_len(format: CaptureFormat) -> Option<usize> {
    match format.encoding {
        CaptureEncoding::Raw(pixel) if format.width % 2 == 0 && format.height % 2 == 0 => {
            Some(pixel.frame_buffer_size(format.width, format.height))
        }
        CaptureEncoding::Raw(_) | CaptureEncoding::Mjpeg => None,
    }
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn enumerate_reports_the_configured_device() {
    let id = device_id();
    let devices = enumerate().unwrap_or_else(|error| panic!("enumerate failed: {error}"));
    let found = devices
        .iter()
        .find(|device| device.id == id)
        .unwrap_or_else(|| {
            panic!(
                "{id:?} is not among the {} enumerated device(s): {:?}",
                devices.len(),
                devices.iter().map(|d| &d.id).collect::<Vec<_>>()
            )
        });
    assert!(!found.name.is_empty(), "a device must report a name");
    assert!(
        !found.formats.is_empty(),
        "a usable device must advertise at least one mode this backend understands"
    );
    for format in &found.formats {
        assert!(format.width > 0 && format.height > 0, "{format}");
    }
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn open_negotiates_a_mode_the_device_advertised() {
    let id = device_id();
    let session = open_configured();
    let negotiated = session.negotiated_format();
    let devices = enumerate().unwrap_or_else(|error| panic!("enumerate failed: {error}"));
    let device = devices
        .iter()
        .find(|device| device.id == id)
        .unwrap_or_else(|| panic!("{id:?} disappeared between open and enumerate"));
    assert!(
        device.formats.contains(&negotiated),
        "negotiated {negotiated}, which the device does not advertise"
    );
    assert_eq!(session.device().id, id);
    assert!(session.is_running());
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn ten_frames_arrive_within_five_seconds() {
    let mut session = open_configured();
    let mut stream = session
        .take_stream()
        .expect("the first take yields a stream");
    let frames = collect(&mut stream, REQUIRED_FRAMES, FRAME_DEADLINE);
    assert_eq!(
        frames.len(),
        REQUIRED_FRAMES,
        "only {} frame(s) in {FRAME_DEADLINE:?}; stats: {:?}",
        frames.len(),
        session.stats()
    );
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn timestamps_never_go_backwards() {
    let mut session = open_configured();
    let mut stream = session
        .take_stream()
        .expect("the first take yields a stream");
    let frames = collect(&mut stream, REQUIRED_FRAMES, FRAME_DEADLINE);
    assert_eq!(frames.len(), REQUIRED_FRAMES);

    if frames[0].sequence == 0 {
        // The very first frame of the session anchors the timeline. A later
        // first frame means the queue evicted ahead of the reader, which is
        // legitimate under `DropOldest` and not something to assert against.
        assert_eq!(
            frames[0].timestamp,
            Duration::ZERO,
            "the first frame anchors the session timeline"
        );
    }
    for pair in frames.windows(2) {
        assert!(
            pair[0].timestamp <= pair[1].timestamp,
            "device timestamp went backwards: {:?} then {:?}",
            pair[0].timestamp,
            pair[1].timestamp
        );
        assert!(
            pair[0].host_timestamp <= pair[1].host_timestamp,
            "host timestamp went backwards: {:?} then {:?}",
            pair[0].host_timestamp,
            pair[1].host_timestamp
        );
        assert!(
            pair[0].sequence < pair[1].sequence,
            "sequence must strictly increase: {} then {}",
            pair[0].sequence,
            pair[1].sequence
        );
    }
    // Ten frames of a live camera span real time; a source stuck at t=0 would
    // pass every check above and still be broken.
    assert!(
        frames[REQUIRED_FRAMES - 1].host_timestamp > Duration::ZERO,
        "the session made no forward progress in host time"
    );
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn payloads_match_the_negotiated_format() {
    let mut session = open_configured();
    let negotiated = session.negotiated_format();
    let mut stream = session
        .take_stream()
        .expect("the first take yields a stream");
    let frames = collect(&mut stream, REQUIRED_FRAMES, FRAME_DEADLINE);
    assert_eq!(frames.len(), REQUIRED_FRAMES);

    for frame in &frames {
        assert_eq!(frame.format, negotiated, "every frame carries its mode");
        assert_eq!(frame.width(), negotiated.width);
        assert_eq!(frame.height(), negotiated.height);

        match (&frame.payload, negotiated.encoding) {
            (FramePayload::Raw(video), CaptureEncoding::Raw(pixel)) => {
                assert_eq!(video.format, pixel);
                assert_eq!(video.width, negotiated.width);
                assert_eq!(video.height, negotiated.height);
                assert_eq!(
                    video.planes.len(),
                    pixel.plane_count() as usize,
                    "{pixel:?} plane count"
                );
                for (index, plane) in video.planes.iter().enumerate() {
                    let stride = pixel
                        .stride_for_width(negotiated.width, index as u32)
                        .unwrap_or_else(|| panic!("no stride for {pixel:?} plane {index}"));
                    assert_eq!(plane.stride, stride, "{pixel:?} plane {index} stride");
                    assert_eq!(
                        plane.data.len(),
                        stride * plane.height as usize,
                        "{pixel:?} plane {index} size"
                    );
                }
            }
            (FramePayload::Compressed(bytes), CaptureEncoding::Mjpeg) => {
                assert!(bytes.len() > 2, "an empty JPEG is not a frame");
                assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG start-of-image marker");
            }
            (payload, encoding) => panic!(
                "payload ({}) does not match the negotiated encoding ({encoding})",
                if payload.is_compressed() {
                    "compressed"
                } else {
                    "raw"
                }
            ),
        }

        if let Some(expected) = expected_payload_len(negotiated) {
            assert_eq!(
                frame.payload.byte_len(),
                expected,
                "{negotiated} frame carries the wrong number of bytes"
            );
        }
    }
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn stop_returns_within_the_shutdown_budget() {
    let mut session = open_configured();
    let mut stream = session
        .take_stream()
        .expect("the first take yields a stream");
    // Stop from a *running* session, not an idle one: the interesting case is
    // tearing down while frames are still arriving.
    assert!(
        !collect(&mut stream, 1, FRAME_DEADLINE).is_empty(),
        "capture never started"
    );

    let started = Instant::now();
    session.stop();
    let elapsed = started.elapsed();
    assert!(!session.is_running());
    assert!(
        elapsed < STOP_BUDGET,
        "stop took {elapsed:?}, budget is {STOP_BUDGET:?}"
    );

    // Idempotent, and still prompt the second time.
    let started = Instant::now();
    session.stop();
    assert!(started.elapsed() < STOP_BUDGET);
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn dropping_a_session_joins_its_thread_promptly() {
    let session = open_configured();
    let started = Instant::now();
    drop(session);
    assert!(
        started.elapsed() < FRAME_DEADLINE,
        "drop took {:?}",
        started.elapsed()
    );
}

#[test]
#[ignore = "needs a real camera; set OXIMEDIA_CAPTURE_DEVICE"]
fn a_live_session_reports_no_transient_errors() {
    let mut session = open_configured();
    let mut stream = session
        .take_stream()
        .expect("the first take yields a stream");
    let frames = collect(&mut stream, REQUIRED_FRAMES, FRAME_DEADLINE);
    assert_eq!(frames.len(), REQUIRED_FRAMES);
    let stats = session.stats();
    assert_eq!(
        stats.errors, 0,
        "a healthy capture reports no errors, got {:?}",
        stats.last_error
    );
    assert!(
        stats.delivered >= REQUIRED_FRAMES as u64,
        "delivered {} frames",
        stats.delivered
    );
    assert_eq!(
        stats.clock_regressions, 0,
        "a live camera clock should not need clamping"
    );
}

/// A guard against the format model drifting away from `oximedia-core`'s
/// buffer sizing. Needs no camera, so it is not `#[ignore]`d.
#[test]
fn the_expected_payload_length_agrees_with_the_core_frame_size() {
    for (pixel, width, height) in [
        (PixelFormat::Nv12, 1280_u32, 720_u32),
        (PixelFormat::Uyvy422, 640, 480),
        (PixelFormat::Yuyv422, 1920, 1080),
    ] {
        let format = CaptureFormat::new(CaptureEncoding::Raw(pixel), width, height, 30, 1);
        assert_eq!(
            expected_payload_len(format),
            Some(pixel.frame_buffer_size(width, height))
        );
    }
    assert_eq!(
        expected_payload_len(CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 1)),
        None
    );
    assert_eq!(
        expected_payload_len(CaptureFormat::new(
            CaptureEncoding::Raw(PixelFormat::Nv12),
            641,
            481,
            30,
            1
        )),
        None,
        "odd dimensions are not checked against the truncating core formula"
    );
}
