//! COM and Media Foundation lifetimes, and the device-activation objects.
//!
//! Media Foundation is COM, and COM's two rules are *per thread* and *paired*:
//! a thread that calls a COM API must have initialized COM on that thread, and
//! every successful initialization needs exactly one matching uninitialization
//! on the same thread. `MFStartup`/`MFShutdown` add a second, independent pair
//! on top. Getting either pair wrong is not a warning — an unbalanced
//! `CoUninitialize` tears down the apartment underneath live objects, and a
//! missing `MFShutdown` leaves the platform's worker threads running for the
//! life of the process.
//!
//! Every one of those pairs is a [`Drop`] impl here, so that no error path in
//! [`super::reader`] can skip one, and the whole file's `unsafe` is spent on
//! COM lifetime management rather than being spread across the backend.
//!
//! # What escapes this module
//!
//! Only safe functions. [`super`] orchestrates enumeration without a single
//! `unsafe` block because every framework call is wrapped here or in
//! [`super::reader`]. Holding, cloning and dropping a `windows` interface value
//! is safe — it is a reference-counted smart pointer — so the wrappers can hand
//! `IMFActivate` values back to safe code; only *calling* a method on one is
//! unsafe, and that never happens outside these two files.

#![allow(unsafe_code)]

use windows::core::{GUID, PWSTR};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaSource, MFCreateAttributes, MFEnumDeviceSources,
    MFShutdown, MFStartup, MFSTARTUP_LITE, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};

use crate::error::CaptureError;

/// `E_ACCESSDENIED`, which is how the Windows camera privacy setting refuses.
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// Turn a `windows` error into a [`CaptureError::Platform`] naming the call.
pub(crate) fn platform_error(call: &str, error: &windows::core::Error) -> CaptureError {
    CaptureError::platform(format!(
        "{call} failed: {:#010x} {}",
        error.code().0 as u32,
        error.message()
    ))
}

/// `true` when this failure is the camera privacy setting rather than a fault.
fn is_access_denied(error: &windows::core::Error) -> bool {
    error.code().0 == E_ACCESSDENIED
}

// ── Platform guard ───────────────────────────────────────────────────────────

/// Which COM apartment this thread ended up in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Apartment {
    /// This guard put the thread into the multi-threaded apartment.
    Multithreaded,
    /// The thread was already in a single-threaded apartment, and stayed there.
    ///
    /// `CoInitializeEx` answered `RPC_E_CHANGED_MODE`, which means it did *not*
    /// initialize anything and did not take a reference — so this guard must
    /// not call `CoUninitialize` either. Media Foundation still works from an
    /// STA thread; the source reader simply marshals more.
    PreexistingSingleThreaded,
}

/// COM and Media Foundation, initialized for the current thread.
///
/// Both are per-thread, so this must be constructed on the thread that will
/// make the calls — for capture, that is inside
/// [`CaptureRunner::run`](crate::backend::CaptureRunner::run), never in
/// `open()`.
#[derive(Debug)]
pub(crate) struct MfGuard {
    apartment: Apartment,
    /// Whether *this* guard's `CoInitializeEx` took a reference that `Drop`
    /// owes a `CoUninitialize` for.
    owns_com: bool,
}

impl MfGuard {
    /// Initialize COM (multi-threaded) and Media Foundation for this thread.
    ///
    /// A thread that some other library already put into a single-threaded
    /// apartment is tolerated rather than refused: `RPC_E_CHANGED_MODE` means
    /// the apartment stays as it was, no reference was taken, and Media
    /// Foundation still functions. Refusing would make this crate unusable
    /// inside any host that runs a message loop.
    pub(crate) fn new() -> Result<Self, CaptureError> {
        // SAFETY: `CoInitializeEx` has no preconditions beyond being called on
        // the thread it is to affect. It returns an `HRESULT` rather than a
        // `Result` precisely because `S_FALSE` and `RPC_E_CHANGED_MODE` are
        // both meaningful non-failures, so the value is inspected rather than
        // `?`-ed.
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let (apartment, owns_com) = if status == RPC_E_CHANGED_MODE {
            tracing::debug!(
                "COM was already initialized on this thread as single-threaded; \
                 Media Foundation will marshal"
            );
            (Apartment::PreexistingSingleThreaded, false)
        } else if status.is_ok() {
            // Covers S_OK (first initialization on this thread) and S_FALSE
            // (already initialized in the same mode). Both take a reference.
            (Apartment::Multithreaded, true)
        } else {
            return Err(CaptureError::platform(format!(
                "CoInitializeEx failed: {:#010x}",
                status.0 as u32
            )));
        };

        // SAFETY: called after COM initialization on this thread, with the
        // version constant the headers define and the lite startup flag, which
        // skips the socket and network-source stacks this crate never uses.
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) } {
            if owns_com {
                // SAFETY: balances the `CoInitializeEx` above, on the same
                // thread, before any COM object has been created.
                unsafe { CoUninitialize() };
            }
            return Err(platform_error("MFStartup", &error));
        }

        Ok(Self {
            apartment,
            owns_com,
        })
    }

    /// Which apartment the thread is running in.
    pub(crate) const fn apartment(&self) -> Apartment {
        self.apartment
    }
}

impl Drop for MfGuard {
    fn drop(&mut self) {
        // SAFETY: balances the `MFStartup` in `new`, on the same thread. Every
        // Media Foundation object this crate creates is dropped before its
        // guard, because the guard is declared first and Rust drops locals in
        // reverse declaration order.
        if let Err(error) = unsafe { MFShutdown() } {
            tracing::warn!(error = %error, "MFShutdown failed");
        }
        if self.owns_com {
            // SAFETY: exactly one call per successful `CoInitializeEx`, on the
            // thread that made it. Skipped entirely when `CoInitializeEx`
            // answered `RPC_E_CHANGED_MODE`, which took no reference.
            unsafe { CoUninitialize() };
        }
    }
}

// ── Task memory ──────────────────────────────────────────────────────────────

/// A block of COM task memory, freed on drop.
///
/// `MFEnumDeviceSources` allocates its result array with `CoTaskMemAlloc`, and
/// the caller owns it. Freeing it by hand would be one early `?` away from a
/// leak on every enumeration.
struct TaskMem(*mut core::ffi::c_void);

impl Drop for TaskMem {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `MFEnumDeviceSources`, which allocates
        // with `CoTaskMemAlloc`, and is freed exactly once because this guard
        // owns it. `CoTaskMemFree` accepts null.
        unsafe { CoTaskMemFree(Some(self.0)) };
    }
}

// ── Strings ──────────────────────────────────────────────────────────────────

/// Copy a `PWSTR` into a `String`, replacing anything that is not valid UTF-16.
///
/// Device names come from the driver's INF file and symbolic links from the
/// kernel; neither is guaranteed well-formed. Lossy conversion keeps a camera
/// with a mangled name usable instead of dropping it, and cannot panic.
///
/// # Safety
///
/// `value` must be null, or point at a NUL-terminated wide string that stays
/// valid for the duration of the call.
unsafe fn pwstr_to_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a NUL-terminated wide string; `as_wide`
    // walks to the terminator and borrows the string without taking ownership.
    let wide = unsafe { value.as_wide() };
    Some(String::from_utf16_lossy(wide))
}

/// Read a string attribute off an activation object.
///
/// Returns `None` when the attribute is absent — which is a normal answer, not
/// a failure: not every capture source carries a friendly name.
pub(crate) fn activate_string(activate: &IMFActivate, key: GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut length = 0_u32;
    // SAFETY: `activate` is a live activation object, `key` is a GUID key, and
    // the two out-parameters point at live locals. On success the string is a
    // fresh `CoTaskMemAlloc` allocation this function owns.
    unsafe { activate.GetAllocatedString(&key, &mut value, &mut length) }.ok()?;
    // SAFETY: `GetAllocatedString` succeeded, so `value` is a NUL-terminated
    // wide string owned by this frame; it is copied before being freed.
    let text = unsafe { pwstr_to_string(value) };
    // SAFETY: the string was allocated by `GetAllocatedString` with
    // `CoTaskMemAlloc` and is freed exactly once, after the copy above.
    unsafe { CoTaskMemFree(Some(value.as_ptr().cast())) };
    text
}

/// The device's human-readable name, if the driver supplies one.
pub(crate) fn friendly_name(activate: &IMFActivate) -> Option<String> {
    activate_string(activate, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
}

/// The device's symbolic link — the stable id this backend hands out.
pub(crate) fn symbolic_link(activate: &IMFActivate) -> Option<String> {
    activate_string(
        activate,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
    )
}

// ── Enumeration ──────────────────────────────────────────────────────────────

/// Every video capture device Media Foundation currently reports.
///
/// Requires a live [`MfGuard`] on this thread, which is why the guard is a
/// parameter rather than something this function creates: enumeration during a
/// capture session must reuse the session's guard instead of nesting a second
/// `MFStartup`/`MFShutdown` pair around it.
pub(crate) fn enumerate_activates(_guard: &MfGuard) -> Result<Vec<IMFActivate>, CaptureError> {
    let mut attributes: Option<IMFAttributes> = None;
    // SAFETY: the out-parameter points at a live local, and one attribute slot
    // is all the source-type key needs.
    unsafe { MFCreateAttributes(&mut attributes, 1) }
        .map_err(|error| platform_error("MFCreateAttributes", &error))?;
    let attributes = attributes.ok_or_else(|| {
        CaptureError::platform("MFCreateAttributes succeeded without producing an attribute store")
    })?;

    // SAFETY: a live attribute store, a GUID key and a GUID value, both of
    // which outlive the call.
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    }
    .map_err(|error| platform_error("IMFAttributes::SetGUID(SOURCE_TYPE)", &error))?;

    let mut activates: *mut Option<IMFActivate> = core::ptr::null_mut();
    let mut count = 0_u32;
    // SAFETY: the attribute store is live and selects video capture sources;
    // both out-parameters point at live locals. On success the array is a
    // `CoTaskMemAlloc` allocation of `count` interface pointers, all owned by
    // this function.
    unsafe { MFEnumDeviceSources(&attributes, &mut activates, &mut count) }
        .map_err(|error| platform_error("MFEnumDeviceSources", &error))?;
    // Owns the array itself from here on, whatever happens below.
    let _array = TaskMem(activates.cast());

    let mut devices = Vec::with_capacity(count as usize);
    if activates.is_null() {
        return Ok(devices);
    }
    for index in 0..count as usize {
        // SAFETY: `activates` points at `count` initialized `Option<IMFActivate>`
        // slots and `index < count`, so this reads one initialized element.
        // `read` *moves* the value out, taking ownership of the reference
        // `MFEnumDeviceSources` handed over; the slot is never read again, so
        // the interface is released exactly once.
        let slot = unsafe { activates.add(index).read() };
        if let Some(activate) = slot {
            devices.push(activate);
        } else {
            tracing::debug!(
                index,
                "MFEnumDeviceSources returned a null activation object"
            );
        }
    }
    Ok(devices)
}

/// Find the activation object whose symbolic link is `device_id`.
///
/// Devices are re-enumerated rather than carried across the `open()` boundary,
/// because a camera can be unplugged between negotiation and the capture
/// thread's first instruction — and because an activation object is a COM
/// pointer, which must not cross threads.
pub(crate) fn find_activate(guard: &MfGuard, device_id: &str) -> Result<IMFActivate, CaptureError> {
    for activate in enumerate_activates(guard)? {
        if symbolic_link(&activate).as_deref() == Some(device_id) {
            return Ok(activate);
        }
    }
    Err(CaptureError::DeviceNotFound(
        crate::device::DeviceSelector::Id(device_id.to_owned()),
    ))
}

// ── Media source ─────────────────────────────────────────────────────────────

/// A live `IMFMediaSource`, shut down on drop.
///
/// `IMFMediaSource::Shutdown` releases the device and the platform resources
/// behind it. Dropping the last reference without it leaves the camera claimed
/// until the process exits, which the next `open()` sees as a device in use.
pub(crate) struct MediaSourceGuard {
    source: IMFMediaSource,
}

impl MediaSourceGuard {
    /// The source, for callers that need to build a reader from it.
    pub(crate) const fn source(&self) -> &IMFMediaSource {
        &self.source
    }
}

impl Drop for MediaSourceGuard {
    fn drop(&mut self) {
        // SAFETY: a live media source that this guard owns the last reference
        // to; `Shutdown` is idempotent from the caller's point of view and is
        // called exactly once here.
        if let Err(error) = unsafe { self.source.Shutdown() } {
            tracing::debug!(error = %error, "IMFMediaSource::Shutdown failed");
        }
    }
}

impl std::fmt::Debug for MediaSourceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaSourceGuard")
    }
}

/// Activate a device's media source.
///
/// Requires a live [`MfGuard`] on the calling thread, which both callers hold
/// for the whole of their work.
///
/// # Errors
///
/// [`CaptureError::PermissionDenied`] when Windows refuses camera access — the
/// per-machine or per-app privacy setting answers `E_ACCESSDENIED` here, and
/// reporting it as a generic platform fault would send users looking for a
/// hardware problem. Everything else is [`CaptureError::Platform`].
pub(crate) fn activate_source(
    activate: &IMFActivate,
    device_id: &str,
) -> Result<MediaSourceGuard, CaptureError> {
    // SAFETY: a live activation object for a video capture device, asked for
    // the interface such a device always implements. The returned source is a
    // +1 reference this frame owns.
    let source = unsafe { activate.ActivateObject::<IMFMediaSource>() }.map_err(|error| {
        if is_access_denied(&error) {
            CaptureError::PermissionDenied {
                device: device_id.to_owned(),
            }
        } else {
            platform_error("IMFActivate::ActivateObject", &error)
        }
    })?;
    Ok(MediaSourceGuard { source })
}
