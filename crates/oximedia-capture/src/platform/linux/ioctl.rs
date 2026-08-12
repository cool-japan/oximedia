//! The fourteen V4L2 ioctls this backend issues, and safe-ish wrappers for them.
//!
//! # Opcodes are derived, not remembered
//!
//! A V4L2 opcode encodes the *size of its argument*: `VIDIOC_S_FMT` is
//! `_IOWR('V', 5, struct v4l2_format)`, and the 208 in
//! `size_of::<v4l2_format>()` is baked into the number the kernel dispatches on.
//! An opcode built from a struct that is one byte off is not a subtly wrong
//! call, it is `ENOTTY` from every driver on the planet.
//!
//! So none of the fourteen numbers below is written down. Each is computed by
//! [`rustix::ioctl::opcode`] from the corresponding [`super::uapi`] type, which
//! makes the opcode a *consequence* of the transcription rather than a second,
//! independently-fallible copy of it. The golden table at the bottom then pins
//! the resulting values against numbers derived from the real
//! `linux/videodev2.h` — see [`super::uapi`] for how — so a drift in either the
//! struct or the opcode direction is a build failure, not a runtime `ENOTTY`.
//!
//! # Direction
//!
//! `_IOC_READ` and `_IOC_WRITE` are named **from userspace's point of view**
//! (`asm-generic/ioctl.h` says so in a comment): `_IOR` means the kernel writes
//! and userspace reads. rustix uses the same convention —
//! [`Direction::Read`](rustix::ioctl::Direction::Read) is `_IOC_READ` = 2,
//! [`Direction::Write`](rustix::ioctl::Direction::Write) is `_IOC_WRITE` = 1 —
//! so `opcode::read` is `_IOR`, `opcode::write` is `_IOW`, and
//! `opcode::read_write` is `_IOWR`. `VIDIOC_STREAMON` is `_IOW('V', 18, int)`:
//! userspace hands the kernel a pointer to an `int` holding the buffer type, so
//! it is a [`Setter`], not a [`Getter`](rustix::ioctl::Getter).
//!
//! # Why `Updater` almost everywhere
//!
//! Every V4L2 ioctl except `STREAMON`/`STREAMOFF` takes a pointer to a struct
//! that carries input fields *and* is filled in by the kernel — `index` in,
//! `pixelformat` out; `count` in, granted `count` out. That is exactly
//! [`rustix::ioctl::Updater`], and using it means this file writes no `as_ptr`
//! and no `MaybeUninit` of its own: every argument is a zero-initialised,
//! fully-owned Rust value that outlives the call. `QUERYCAP` is `_IOR` and could
//! use `Getter`, but `Getter` reads a `MaybeUninit` back out, so it is driven
//! through `Updater` as well — the opcode is what the kernel dispatches on, and
//! starting from zeroed memory means a driver that fills fewer bytes than it
//! promises yields zeros rather than whatever was on the stack.

#![allow(unsafe_code)]

use core::ffi::c_int;

use rustix::fd::BorrowedFd;
use rustix::io::Errno;
use rustix::ioctl::{opcode, Opcode, Setter, Updater};

use crate::error::CaptureError;

use super::uapi::{
    v4l2_buffer, v4l2_capability, v4l2_fmtdesc, v4l2_format, v4l2_frmivalenum, v4l2_frmsizeenum,
    v4l2_requestbuffers, v4l2_streamparm, V4L2_BUF_TYPE_VIDEO_CAPTURE, V4L2_MEMORY_MMAP,
};

// ── Opcodes ──────────────────────────────────────────────────────────────────

/// `VIDIOC_QUERYCAP` — `_IOR('V', 0, struct v4l2_capability)`.
const VIDIOC_QUERYCAP: Opcode = opcode::read::<v4l2_capability>(b'V', 0);
/// `VIDIOC_ENUM_FMT` — `_IOWR('V', 2, struct v4l2_fmtdesc)`.
const VIDIOC_ENUM_FMT: Opcode = opcode::read_write::<v4l2_fmtdesc>(b'V', 2);
/// `VIDIOC_G_FMT` — `_IOWR('V', 4, struct v4l2_format)`.
const VIDIOC_G_FMT: Opcode = opcode::read_write::<v4l2_format>(b'V', 4);
/// `VIDIOC_S_FMT` — `_IOWR('V', 5, struct v4l2_format)`.
const VIDIOC_S_FMT: Opcode = opcode::read_write::<v4l2_format>(b'V', 5);
/// `VIDIOC_REQBUFS` — `_IOWR('V', 8, struct v4l2_requestbuffers)`.
const VIDIOC_REQBUFS: Opcode = opcode::read_write::<v4l2_requestbuffers>(b'V', 8);
/// `VIDIOC_QUERYBUF` — `_IOWR('V', 9, struct v4l2_buffer)`.
const VIDIOC_QUERYBUF: Opcode = opcode::read_write::<v4l2_buffer>(b'V', 9);
/// `VIDIOC_QBUF` — `_IOWR('V', 15, struct v4l2_buffer)`.
const VIDIOC_QBUF: Opcode = opcode::read_write::<v4l2_buffer>(b'V', 15);
/// `VIDIOC_DQBUF` — `_IOWR('V', 17, struct v4l2_buffer)`.
const VIDIOC_DQBUF: Opcode = opcode::read_write::<v4l2_buffer>(b'V', 17);
/// `VIDIOC_STREAMON` — `_IOW('V', 18, int)`.
const VIDIOC_STREAMON: Opcode = opcode::write::<c_int>(b'V', 18);
/// `VIDIOC_STREAMOFF` — `_IOW('V', 19, int)`.
const VIDIOC_STREAMOFF: Opcode = opcode::write::<c_int>(b'V', 19);
/// `VIDIOC_G_PARM` — `_IOWR('V', 21, struct v4l2_streamparm)`.
const VIDIOC_G_PARM: Opcode = opcode::read_write::<v4l2_streamparm>(b'V', 21);
/// `VIDIOC_S_PARM` — `_IOWR('V', 22, struct v4l2_streamparm)`.
const VIDIOC_S_PARM: Opcode = opcode::read_write::<v4l2_streamparm>(b'V', 22);
/// `VIDIOC_ENUM_FRAMESIZES` — `_IOWR('V', 74, struct v4l2_frmsizeenum)`.
const VIDIOC_ENUM_FRAMESIZES: Opcode = opcode::read_write::<v4l2_frmsizeenum>(b'V', 74);
/// `VIDIOC_ENUM_FRAMEINTERVALS` — `_IOWR('V', 75, struct v4l2_frmivalenum)`.
const VIDIOC_ENUM_FRAMEINTERVALS: Opcode = opcode::read_write::<v4l2_frmivalenum>(b'V', 75);

// ── Wrappers ─────────────────────────────────────────────────────────────────

/// Issue an updating ioctl: the kernel reads `value` and writes back into it.
///
/// # Safety
///
/// `OPCODE` must be a V4L2 opcode whose argument type is exactly `T`. Every
/// caller in this file satisfies that by construction — each opcode above is
/// built with `opcode::read_write::<T>` for the same `T` the wrapper passes —
/// and this function exists so that the `unsafe` needed to say so is written
/// once instead of thirteen times.
unsafe fn update<const OPCODE: Opcode, T>(fd: BorrowedFd<'_>, value: &mut T) -> Result<(), Errno> {
    // SAFETY: the caller guarantees that `OPCODE`'s argument type is `T`.
    // `value` is a live, fully-initialised `T` that outlives the call, which is
    // what `Updater` requires, and `Updater` marks itself as mutating so rustix
    // does not apply its read-only-ioctl optimisation to a call that writes.
    unsafe { rustix::ioctl::ioctl(fd, Updater::<OPCODE, T>::new(value)) }
}

/// `VIDIOC_QUERYCAP`: what kind of device is this, and what can it do?
pub(crate) fn querycap(fd: BorrowedFd<'_>) -> Result<v4l2_capability, Errno> {
    let mut caps = v4l2_capability::default();
    // SAFETY: `VIDIOC_QUERYCAP` is built from `v4l2_capability`, which is what
    // is passed. (`_IOR` rather than `_IOWR` only means the kernel ignores the
    // incoming bytes; passing zeroed ones is always valid.)
    unsafe { update::<VIDIOC_QUERYCAP, _>(fd, &mut caps) }?;
    Ok(caps)
}

/// `VIDIOC_ENUM_FMT`: the `index`-th pixel format the device offers.
///
/// The walk ends when the kernel answers [`Errno::INVAL`]; that is the
/// documented terminator, not a failure.
pub(crate) fn enum_fmt(fd: BorrowedFd<'_>, index: u32) -> Result<v4l2_fmtdesc, Errno> {
    let mut desc = v4l2_fmtdesc {
        index,
        type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        ..v4l2_fmtdesc::default()
    };
    // SAFETY: `VIDIOC_ENUM_FMT` is built from `v4l2_fmtdesc`, which is passed.
    unsafe { update::<VIDIOC_ENUM_FMT, _>(fd, &mut desc) }?;
    Ok(desc)
}

/// `VIDIOC_G_FMT`: the format the device is currently configured for.
pub(crate) fn g_fmt(fd: BorrowedFd<'_>) -> Result<v4l2_format, Errno> {
    let mut format = v4l2_format {
        type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        ..v4l2_format::default()
    };
    // SAFETY: `VIDIOC_G_FMT` is built from `v4l2_format`, which is passed.
    unsafe { update::<VIDIOC_G_FMT, _>(fd, &mut format) }?;
    Ok(format)
}

/// `VIDIOC_S_FMT`: ask the device for a format.
///
/// V4L2 negotiation is *echo-based*: the driver overwrites `format` with what it
/// will actually deliver and reports success even when that differs from the
/// request. The caller must compare, which is why this returns the echoed value
/// rather than `()`.
pub(crate) fn s_fmt(fd: BorrowedFd<'_>, format: v4l2_format) -> Result<v4l2_format, Errno> {
    let mut echoed = format;
    // SAFETY: `VIDIOC_S_FMT` is built from `v4l2_format`, which is passed.
    unsafe { update::<VIDIOC_S_FMT, _>(fd, &mut echoed) }?;
    Ok(echoed)
}

/// `VIDIOC_G_PARM`: the device's current streaming parameters.
pub(crate) fn g_parm(fd: BorrowedFd<'_>) -> Result<v4l2_streamparm, Errno> {
    let mut parm = v4l2_streamparm::query();
    // SAFETY: `VIDIOC_G_PARM` is built from `v4l2_streamparm`, which is passed.
    unsafe { update::<VIDIOC_G_PARM, _>(fd, &mut parm) }?;
    Ok(parm)
}

/// `VIDIOC_S_PARM`: ask the device for a frame interval.
///
/// Echo-based like [`s_fmt`]; the granted interval comes back in the result.
pub(crate) fn s_parm(fd: BorrowedFd<'_>, parm: v4l2_streamparm) -> Result<v4l2_streamparm, Errno> {
    let mut echoed = parm;
    // SAFETY: `VIDIOC_S_PARM` is built from `v4l2_streamparm`, which is passed.
    unsafe { update::<VIDIOC_S_PARM, _>(fd, &mut echoed) }?;
    Ok(echoed)
}

/// `VIDIOC_REQBUFS`: allocate the driver's buffer ring.
///
/// Returns the *granted* count, which the kernel is allowed to make smaller —
/// or, for `count == 0`, larger than zero is impossible and the caller must
/// treat it as a refusal.
pub(crate) fn reqbufs(fd: BorrowedFd<'_>, count: u32) -> Result<v4l2_requestbuffers, Errno> {
    let mut request = v4l2_requestbuffers {
        count,
        type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: V4L2_MEMORY_MMAP,
        ..v4l2_requestbuffers::default()
    };
    // SAFETY: `VIDIOC_REQBUFS` is built from `v4l2_requestbuffers`, passed here.
    unsafe { update::<VIDIOC_REQBUFS, _>(fd, &mut request) }?;
    Ok(request)
}

/// `VIDIOC_QUERYBUF`: where in the driver's memory buffer `index` lives.
pub(crate) fn querybuf(fd: BorrowedFd<'_>, index: u32) -> Result<v4l2_buffer, Errno> {
    let mut buffer = v4l2_buffer::mmap(index);
    // SAFETY: `VIDIOC_QUERYBUF` is built from `v4l2_buffer`, which is passed.
    unsafe { update::<VIDIOC_QUERYBUF, _>(fd, &mut buffer) }?;
    Ok(buffer)
}

/// `VIDIOC_QBUF`: hand buffer `index` back to the driver to be filled.
pub(crate) fn qbuf(fd: BorrowedFd<'_>, index: u32) -> Result<(), Errno> {
    let mut buffer = v4l2_buffer::mmap(index);
    // SAFETY: `VIDIOC_QBUF` is built from `v4l2_buffer`, which is passed.
    unsafe { update::<VIDIOC_QBUF, _>(fd, &mut buffer) }
}

/// `VIDIOC_DQBUF`: take the oldest filled buffer off the driver's queue.
///
/// Blocks unless the descriptor is non-blocking; this backend always calls it
/// after a `poll` has said a buffer is ready, so it does not block in practice.
/// [`Errno::AGAIN`] (nothing ready after all) and [`Errno::INTR`] (a signal) are
/// both ordinary outcomes the caller retries.
pub(crate) fn dqbuf(fd: BorrowedFd<'_>) -> Result<v4l2_buffer, Errno> {
    let mut buffer = v4l2_buffer {
        type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: V4L2_MEMORY_MMAP,
        ..v4l2_buffer::default()
    };
    // SAFETY: `VIDIOC_DQBUF` is built from `v4l2_buffer`, which is passed.
    unsafe { update::<VIDIOC_DQBUF, _>(fd, &mut buffer) }?;
    Ok(buffer)
}

/// `VIDIOC_STREAMON`: start filling buffers.
pub(crate) fn streamon(fd: BorrowedFd<'_>) -> Result<(), Errno> {
    // SAFETY: `VIDIOC_STREAMON` is `_IOW('V', 18, int)`, so the kernel expects a
    // pointer to a single `int` holding an `enum v4l2_buf_type`. `Setter` passes
    // a pointer to exactly that, and the value is the capture buffer type this
    // backend streams.
    unsafe {
        rustix::ioctl::ioctl(
            fd,
            Setter::<VIDIOC_STREAMON, c_int>::new(V4L2_BUF_TYPE_VIDEO_CAPTURE as c_int),
        )
    }
}

/// `VIDIOC_STREAMOFF`: stop filling buffers, and drop everything queued.
///
/// The kernel also un-queues every buffer, so nothing has to be dequeued first.
pub(crate) fn streamoff(fd: BorrowedFd<'_>) -> Result<(), Errno> {
    // SAFETY: as `streamon`; `_IOW('V', 19, int)` with the same argument.
    unsafe {
        rustix::ioctl::ioctl(
            fd,
            Setter::<VIDIOC_STREAMOFF, c_int>::new(V4L2_BUF_TYPE_VIDEO_CAPTURE as c_int),
        )
    }
}

/// `VIDIOC_ENUM_FRAMESIZES`: the `index`-th frame size offered for `code`.
pub(crate) fn enum_framesizes(
    fd: BorrowedFd<'_>,
    index: u32,
    code: u32,
) -> Result<v4l2_frmsizeenum, Errno> {
    let mut sizes = v4l2_frmsizeenum::query(index, code);
    // SAFETY: `VIDIOC_ENUM_FRAMESIZES` is built from `v4l2_frmsizeenum`, passed.
    unsafe { update::<VIDIOC_ENUM_FRAMESIZES, _>(fd, &mut sizes) }?;
    Ok(sizes)
}

/// `VIDIOC_ENUM_FRAMEINTERVALS`: the `index`-th interval offered for `code` at
/// `width` × `height`.
pub(crate) fn enum_frameintervals(
    fd: BorrowedFd<'_>,
    index: u32,
    code: u32,
    width: u32,
    height: u32,
) -> Result<v4l2_frmivalenum, Errno> {
    let mut intervals = v4l2_frmivalenum::query(index, code, width, height);
    // SAFETY: `VIDIOC_ENUM_FRAMEINTERVALS` is built from `v4l2_frmivalenum`,
    // which is what is passed.
    unsafe { update::<VIDIOC_ENUM_FRAMEINTERVALS, _>(fd, &mut intervals) }?;
    Ok(intervals)
}

// ── Error translation ────────────────────────────────────────────────────────

/// Turn a failed ioctl into a [`CaptureError`] that names what was attempted.
///
/// `EACCES`/`EPERM` become [`CaptureError::PermissionDenied`], because on Linux
/// that is almost always group membership on the `/dev/video*` node and saying
/// "I/O error" would send the reader looking at the camera instead of at
/// `usermod -aG video`. Everything else keeps the operating system's own message
/// — including the errno number — behind a
/// [`CaptureError::Platform`](crate::CaptureError::Platform) that names the
/// ioctl, because "VIDIOC_S_FMT failed: Invalid argument (os error 22)" is a
/// diagnosis and "I/O error" is not.
pub(crate) fn ioctl_error(device: &str, operation: &str, errno: Errno) -> CaptureError {
    if errno == Errno::ACCESS || errno == Errno::PERM {
        return CaptureError::PermissionDenied {
            device: device.to_owned(),
        };
    }
    CaptureError::platform(format!(
        "{operation} on {device:?} failed: {}",
        std::io::Error::from_raw_os_error(errno.raw_os_error())
    ))
}

// ── Golden opcode table ──────────────────────────────────────────────────────
//
// DERIVED, NOT REMEMBERED — same procedure and same header as the layout table
// in `super::uapi`: `zig translate-c` of `#include <linux/videodev2.h>` for four
// Linux triples, then the translated `VIDIOC_*` macro expansions evaluated at
// Zig comptime. The five opcodes that differ between the two arms are exactly
// the five whose argument is `v4l2_format` or `v4l2_buffer`, i.e. the two
// structs whose size depends on the pointer width.
//
// Compared as `u32` because `rustix::ioctl::Opcode` is `c_uint` under rustix's
// `linux_raw` backend but `c_ulong` (or `c_int`) under its libc backend; the
// low 32 bits are the opcode either way.

/// Expected opcode values on a 64-bit Linux ABI.
#[cfg(target_pointer_width = "64")]
mod expected {
    /// `(name, value)` for each of the fourteen ioctls.
    pub(super) const OPCODES: [(&str, u32); 14] = [
        ("VIDIOC_QUERYCAP", 0x8068_5600),
        ("VIDIOC_ENUM_FMT", 0xC040_5602),
        ("VIDIOC_G_FMT", 0xC0D0_5604),
        ("VIDIOC_S_FMT", 0xC0D0_5605),
        ("VIDIOC_REQBUFS", 0xC014_5608),
        ("VIDIOC_QUERYBUF", 0xC058_5609),
        ("VIDIOC_QBUF", 0xC058_560F),
        ("VIDIOC_DQBUF", 0xC058_5611),
        ("VIDIOC_STREAMON", 0x4004_5612),
        ("VIDIOC_STREAMOFF", 0x4004_5613),
        ("VIDIOC_G_PARM", 0xC0CC_5615),
        ("VIDIOC_S_PARM", 0xC0CC_5616),
        ("VIDIOC_ENUM_FRAMESIZES", 0xC02C_564A),
        ("VIDIOC_ENUM_FRAMEINTERVALS", 0xC034_564B),
    ];
}

/// Expected opcode values on a 32-bit Linux ABI.
///
/// `G_FMT`/`S_FMT` shrink by 4 (`v4l2_format` 208 → 204) and the three
/// `v4l2_buffer` ioctls by 20 (88 → 68); the rest are identical.
#[cfg(not(target_pointer_width = "64"))]
mod expected {
    /// `(name, value)` for each of the fourteen ioctls.
    pub(super) const OPCODES: [(&str, u32); 14] = [
        ("VIDIOC_QUERYCAP", 0x8068_5600),
        ("VIDIOC_ENUM_FMT", 0xC040_5602),
        ("VIDIOC_G_FMT", 0xC0CC_5604),
        ("VIDIOC_S_FMT", 0xC0CC_5605),
        ("VIDIOC_REQBUFS", 0xC014_5608),
        ("VIDIOC_QUERYBUF", 0xC044_5609),
        ("VIDIOC_QBUF", 0xC044_560F),
        ("VIDIOC_DQBUF", 0xC044_5611),
        ("VIDIOC_STREAMON", 0x4004_5612),
        ("VIDIOC_STREAMOFF", 0x4004_5613),
        ("VIDIOC_G_PARM", 0xC0CC_5615),
        ("VIDIOC_S_PARM", 0xC0CC_5616),
        ("VIDIOC_ENUM_FRAMESIZES", 0xC02C_564A),
        ("VIDIOC_ENUM_FRAMEINTERVALS", 0xC034_564B),
    ];
}

/// The opcodes this file actually computes, in [`expected::OPCODES`] order.
const ACTUAL_OPCODES: [(&str, u32); 14] = [
    ("VIDIOC_QUERYCAP", VIDIOC_QUERYCAP as u32),
    ("VIDIOC_ENUM_FMT", VIDIOC_ENUM_FMT as u32),
    ("VIDIOC_G_FMT", VIDIOC_G_FMT as u32),
    ("VIDIOC_S_FMT", VIDIOC_S_FMT as u32),
    ("VIDIOC_REQBUFS", VIDIOC_REQBUFS as u32),
    ("VIDIOC_QUERYBUF", VIDIOC_QUERYBUF as u32),
    ("VIDIOC_QBUF", VIDIOC_QBUF as u32),
    ("VIDIOC_DQBUF", VIDIOC_DQBUF as u32),
    ("VIDIOC_STREAMON", VIDIOC_STREAMON as u32),
    ("VIDIOC_STREAMOFF", VIDIOC_STREAMOFF as u32),
    ("VIDIOC_G_PARM", VIDIOC_G_PARM as u32),
    ("VIDIOC_S_PARM", VIDIOC_S_PARM as u32),
    ("VIDIOC_ENUM_FRAMESIZES", VIDIOC_ENUM_FRAMESIZES as u32),
    (
        "VIDIOC_ENUM_FRAMEINTERVALS",
        VIDIOC_ENUM_FRAMEINTERVALS as u32,
    ),
];

const _: () = {
    let mut index = 0;
    while index < ACTUAL_OPCODES.len() {
        assert!(
            ACTUAL_OPCODES[index].1 == expected::OPCODES[index].1,
            "a V4L2 opcode does not match the kernel's; see the golden table"
        );
        index += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit positions from `asm-generic/ioctl.h`, for the decomposition below.
    const NR_SHIFT: u32 = 0;
    const TYPE_SHIFT: u32 = 8;
    const SIZE_SHIFT: u32 = 16;
    const DIR_SHIFT: u32 = 30;
    const IOC_WRITE: u32 = 1;
    const IOC_READ: u32 = 2;

    #[test]
    fn every_opcode_matches_the_one_the_kernel_headers_encode() {
        for (actual, wanted) in ACTUAL_OPCODES.iter().zip(expected::OPCODES.iter()) {
            assert_eq!(
                actual.0, wanted.0,
                "the two tables must list the same ioctls"
            );
            assert_eq!(actual.1, wanted.1, "{} is {:#010X}", actual.0, actual.1);
        }
    }

    /// Decompose each opcode and check the four fields independently, so a
    /// failure says *which* part drifted rather than just "the number changed".
    #[test]
    fn opcodes_decompose_into_the_documented_fields() {
        let table = [
            (
                "VIDIOC_QUERYCAP",
                VIDIOC_QUERYCAP as u32,
                IOC_READ,
                0_u32,
                size_of::<v4l2_capability>(),
            ),
            (
                "VIDIOC_ENUM_FMT",
                VIDIOC_ENUM_FMT as u32,
                IOC_READ | IOC_WRITE,
                2,
                size_of::<v4l2_fmtdesc>(),
            ),
            (
                "VIDIOC_G_FMT",
                VIDIOC_G_FMT as u32,
                IOC_READ | IOC_WRITE,
                4,
                size_of::<v4l2_format>(),
            ),
            (
                "VIDIOC_S_FMT",
                VIDIOC_S_FMT as u32,
                IOC_READ | IOC_WRITE,
                5,
                size_of::<v4l2_format>(),
            ),
            (
                "VIDIOC_REQBUFS",
                VIDIOC_REQBUFS as u32,
                IOC_READ | IOC_WRITE,
                8,
                size_of::<v4l2_requestbuffers>(),
            ),
            (
                "VIDIOC_QUERYBUF",
                VIDIOC_QUERYBUF as u32,
                IOC_READ | IOC_WRITE,
                9,
                size_of::<v4l2_buffer>(),
            ),
            (
                "VIDIOC_QBUF",
                VIDIOC_QBUF as u32,
                IOC_READ | IOC_WRITE,
                15,
                size_of::<v4l2_buffer>(),
            ),
            (
                "VIDIOC_DQBUF",
                VIDIOC_DQBUF as u32,
                IOC_READ | IOC_WRITE,
                17,
                size_of::<v4l2_buffer>(),
            ),
            (
                "VIDIOC_STREAMON",
                VIDIOC_STREAMON as u32,
                IOC_WRITE,
                18,
                size_of::<c_int>(),
            ),
            (
                "VIDIOC_STREAMOFF",
                VIDIOC_STREAMOFF as u32,
                IOC_WRITE,
                19,
                size_of::<c_int>(),
            ),
            (
                "VIDIOC_G_PARM",
                VIDIOC_G_PARM as u32,
                IOC_READ | IOC_WRITE,
                21,
                size_of::<v4l2_streamparm>(),
            ),
            (
                "VIDIOC_S_PARM",
                VIDIOC_S_PARM as u32,
                IOC_READ | IOC_WRITE,
                22,
                size_of::<v4l2_streamparm>(),
            ),
            (
                "VIDIOC_ENUM_FRAMESIZES",
                VIDIOC_ENUM_FRAMESIZES as u32,
                IOC_READ | IOC_WRITE,
                74,
                size_of::<v4l2_frmsizeenum>(),
            ),
            (
                "VIDIOC_ENUM_FRAMEINTERVALS",
                VIDIOC_ENUM_FRAMEINTERVALS as u32,
                IOC_READ | IOC_WRITE,
                75,
                size_of::<v4l2_frmivalenum>(),
            ),
        ];
        for (name, opcode, direction, number, size) in table {
            assert_eq!((opcode >> DIR_SHIFT) & 0x3, direction, "{name} direction");
            assert_eq!(
                (opcode >> TYPE_SHIFT) & 0xFF,
                u32::from(b'V'),
                "{name} group"
            );
            assert_eq!((opcode >> NR_SHIFT) & 0xFF, number, "{name} number");
            assert_eq!(
                (opcode >> SIZE_SHIFT) & 0x3FFF,
                size as u32,
                "{name} argument size"
            );
        }
    }

    /// `_IOR` is "userspace reads", which is the opposite of what the name
    /// suggests to anyone who has not read `asm-generic/ioctl.h`. Getting it
    /// backwards swaps `QUERYCAP` (0x8068…) with an `_IOW` (0x4068…), so it is
    /// asserted on its own.
    #[test]
    fn the_direction_bits_follow_the_kernels_userspace_relative_convention() {
        assert_eq!((VIDIOC_QUERYCAP as u32) >> DIR_SHIFT, IOC_READ, "_IOR");
        assert_eq!((VIDIOC_STREAMON as u32) >> DIR_SHIFT, IOC_WRITE, "_IOW");
        assert_eq!(
            (VIDIOC_S_FMT as u32) >> DIR_SHIFT,
            IOC_READ | IOC_WRITE,
            "_IOWR"
        );
    }

    #[test]
    fn all_fourteen_opcodes_are_distinct() {
        for (index, (name, opcode)) in ACTUAL_OPCODES.iter().enumerate() {
            for (other_name, other) in &ACTUAL_OPCODES[index + 1..] {
                assert_ne!(opcode, other, "{name} and {other_name} collide");
            }
        }
    }

    #[test]
    fn permission_errors_are_reported_as_permission_errors() {
        for errno in [Errno::ACCESS, Errno::PERM] {
            match ioctl_error("/dev/video0", "VIDIOC_QUERYCAP", errno) {
                CaptureError::PermissionDenied { device } => assert_eq!(device, "/dev/video0"),
                other => panic!("expected PermissionDenied for {errno:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn other_errors_keep_the_operating_systems_own_diagnosis() {
        let error = ioctl_error("/dev/video0", "VIDIOC_S_FMT", Errno::INVAL);
        let text = error.to_string();
        assert!(matches!(error, CaptureError::Platform(_)), "{error:?}");
        assert!(text.contains("VIDIOC_S_FMT"), "{text}");
        assert!(text.contains("/dev/video0"), "{text}");
        // The errno number survives, so a report can be matched against
        // `errno.h` without guessing.
        assert!(text.contains("os error 22"), "{text}");
    }

    /// The ioctl that says "this driver does not implement that call" must not
    /// be mistaken for a permission problem, because the fix is completely
    /// different.
    #[test]
    fn an_unimplemented_ioctl_is_not_a_permission_problem() {
        let error = ioctl_error("/dev/video0", "VIDIOC_ENUM_FRAMESIZES", Errno::NOTTY);
        assert!(matches!(error, CaptureError::Platform(_)), "{error:?}");
    }
}
