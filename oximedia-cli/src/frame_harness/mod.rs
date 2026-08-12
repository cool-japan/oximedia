//! Frame-processing harness: a real decode → process → encode path for
//! `oximedia-cli` commands that need pixels rather than packets.
//!
//! # Why Y4M
//!
//! Every command that reaches for this harness must *rewrite pixels*, which
//! makes it a transcode rather than a container remux. Rather than depending
//! on a particular compressed-codec encoder (the VP8/VP9/AV1 encoders in this
//! tree do not yet emit valid bitstreams), the harness fixes its I/O contract
//! at uncompressed YUV4MPEG2: a lossless, self-describing container that the
//! OxiMedia stack demuxes and muxes bit-exactly. Non-Y4M inputs are rejected
//! with an actionable error naming the conversion command.
//!
//! # Entry points
//!
//! * [`process_clip`] — whole-clip operations that need global context (e.g.
//!   stabilisation, which derives one motion trajectory across every frame).
//! * [`process_frames`] — streaming, frame-at-a-time operations (e.g. overlay
//!   burn-in). Frames are demuxed, mutated and muxed one at a time.
//!
//! Both return [`ClipStats`], whose `bytes_changed` field counts frame-data
//! bytes that actually differ between input and output. It is the harness's
//! anti-fabrication counter: a wiring test for an operation that *must* change
//! pixels asserts `bytes_changed > 0`, so a silently-identity pipeline cannot
//! pass as working.
//!
//! # Output atomicity
//!
//! The muxed stream is buffered in memory and written with a single
//! `std::fs::write` after the whole clip succeeds. A failing operation
//! therefore leaves no partial output file behind — honesty tests assert
//! exactly that.

pub mod adapt;
pub mod font;
pub mod ops;
pub mod scale;
pub mod text;

use anyhow::{Context, Result};
use oximedia_container::demux::y4m::{Y4mChroma, Y4mDemuxer, Y4mHeader};
use oximedia_container::mux::y4m::Y4mMuxerBuilder;
use std::path::Path;

/// Y4M magic bytes at the start of every YUV4MPEG2 stream.
const Y4M_MAGIC: &[u8] = b"YUV4MPEG2";

// ---------------------------------------------------------------------------
// Plane geometry
// ---------------------------------------------------------------------------

/// Plane geometry of a Y4M frame for a particular chroma subsampling.
///
/// Lifted verbatim (plus the `chroma` tag and plane helpers) from the private
/// copy that used to live in `restore_cmd.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromaLayout {
    /// Luma plane width.
    pub luma_w: usize,
    /// Luma plane height.
    pub luma_h: usize,
    /// Chroma plane width (0 if there are no chroma planes).
    pub chroma_w: usize,
    /// Chroma plane height (0 if there are no chroma planes).
    pub chroma_h: usize,
    /// Whether the frame carries a full-resolution alpha plane.
    pub has_alpha: bool,
    /// The Y4M chroma tag this layout was derived from.
    pub chroma: Y4mChroma,
}

impl ChromaLayout {
    /// Derive the plane layout for a Y4M chroma format and frame size.
    ///
    /// Returns `None` for chroma formats this pipeline does not handle.
    #[must_use]
    pub fn for_chroma(chroma: Y4mChroma, width: u32, height: u32) -> Option<Self> {
        let luma_w = width as usize;
        let luma_h = height as usize;
        let (chroma_w, chroma_h, has_alpha) = match chroma {
            Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => {
                (luma_w.div_ceil(2), luma_h.div_ceil(2), false)
            }
            Y4mChroma::C422 => (luma_w.div_ceil(2), luma_h, false),
            Y4mChroma::C444 => (luma_w, luma_h, false),
            Y4mChroma::C444alpha => (luma_w, luma_h, true),
            Y4mChroma::Mono => (0, 0, false),
        };
        Some(Self {
            luma_w,
            luma_h,
            chroma_w,
            chroma_h,
            has_alpha,
            chroma,
        })
    }

    /// Total bytes in one packed planar frame.
    #[must_use]
    pub const fn frame_size(&self) -> usize {
        self.luma_len() + 2 * self.chroma_len() + if self.has_alpha { self.luma_len() } else { 0 }
    }

    /// Bytes in the luma (and alpha) plane.
    #[must_use]
    pub const fn luma_len(&self) -> usize {
        self.luma_w * self.luma_h
    }

    /// Bytes in one chroma plane (0 for monochrome).
    #[must_use]
    pub const fn chroma_len(&self) -> usize {
        self.chroma_w * self.chroma_h
    }

    /// Number of packed planes in a frame (1, 3 or 4).
    #[must_use]
    pub const fn plane_count(&self) -> usize {
        let base = if self.chroma_len() == 0 { 1 } else { 3 };
        if self.has_alpha {
            base + 1
        } else {
            base
        }
    }

    /// Byte offset of plane `index` inside a packed frame.
    ///
    /// Returns `None` when `index` is beyond [`Self::plane_count`].
    #[must_use]
    pub const fn plane_offset(&self, index: usize) -> Option<usize> {
        match index {
            0 => Some(0),
            1 if self.chroma_len() > 0 => Some(self.luma_len()),
            2 if self.chroma_len() > 0 => Some(self.luma_len() + self.chroma_len()),
            i if self.has_alpha && i == self.plane_count() - 1 => {
                Some(self.luma_len() + 2 * self.chroma_len())
            }
            _ => None,
        }
    }

    /// Pixel dimensions `(width, height)` of plane `index`.
    #[must_use]
    pub const fn plane_dims(&self, index: usize) -> Option<(usize, usize)> {
        match index {
            0 => Some((self.luma_w, self.luma_h)),
            1 | 2 if self.chroma_len() > 0 => Some((self.chroma_w, self.chroma_h)),
            i if self.has_alpha && i == self.plane_count() - 1 => Some((self.luma_w, self.luma_h)),
            _ => None,
        }
    }

    /// Byte length of plane `index`.
    #[must_use]
    pub fn plane_len(&self, index: usize) -> Option<usize> {
        self.plane_dims(index).map(|(w, h)| w * h)
    }
}

// ---------------------------------------------------------------------------
// Planar frames and clips
// ---------------------------------------------------------------------------

/// One packed planar-YUV frame plus the geometry needed to interpret it.
#[derive(Debug, Clone)]
pub struct PlanarFrame {
    /// Plane geometry (shared with the clip this frame came from).
    pub layout: ChromaLayout,
    /// Packed plane bytes: Y, then Cb, Cr, then optional alpha.
    pub data: Vec<u8>,
}

impl PlanarFrame {
    /// Wrap packed plane bytes, validating the length against `layout`.
    ///
    /// # Errors
    ///
    /// Returns an error if `data` does not match the layout's frame size.
    pub fn new(layout: ChromaLayout, data: Vec<u8>) -> Result<Self> {
        let expected = layout.frame_size();
        if data.len() != expected {
            anyhow::bail!(
                "planar frame has {} bytes, expected {expected} for {}x{} {:?}",
                data.len(),
                layout.luma_w,
                layout.luma_h,
                layout.chroma
            );
        }
        Ok(Self { layout, data })
    }

    /// Immutable view of plane `index`.
    #[must_use]
    pub fn plane(&self, index: usize) -> Option<&[u8]> {
        let offset = self.layout.plane_offset(index)?;
        let len = self.layout.plane_len(index)?;
        self.data.get(offset..offset + len)
    }

    /// Mutable view of plane `index`.
    pub fn plane_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        let offset = self.layout.plane_offset(index)?;
        let len = self.layout.plane_len(index)?;
        self.data.get_mut(offset..offset + len)
    }

    /// Immutable view of the luma plane.
    #[must_use]
    pub fn luma(&self) -> &[u8] {
        &self.data[..self.layout.luma_len()]
    }

    /// Mutable view of the luma plane.
    pub fn luma_mut(&mut self) -> &mut [u8] {
        let len = self.layout.luma_len();
        &mut self.data[..len]
    }
}

/// A whole decoded Y4M clip held in memory.
#[derive(Debug, Clone)]
pub struct PlanarClip {
    /// Plane geometry shared by every frame.
    pub layout: ChromaLayout,
    /// The source Y4M header (frame rate, interlace, aspect ratio, chroma).
    pub header: Y4mHeader,
    /// Decoded frames in presentation order.
    pub frames: Vec<PlanarFrame>,
}

impl PlanarClip {
    /// Build a clip from a header and packed frame buffers.
    ///
    /// # Errors
    ///
    /// Returns an error if the header's chroma format is unsupported or if any
    /// frame's length disagrees with the declared geometry.
    pub fn from_raw_frames(header: Y4mHeader, frames_raw: Vec<Vec<u8>>) -> Result<Self> {
        let layout = layout_for_header(&header)?;
        let frames = frames_raw
            .into_iter()
            .enumerate()
            .map(|(i, raw)| {
                PlanarFrame::new(layout, raw)
                    .with_context(|| format!("Y4M frame {i} has an unexpected size"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            layout,
            header,
            frames,
        })
    }

    /// Number of frames in the clip.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the clip has no frames.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.header.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.header.height
    }

    /// Build an empty clip that shares this clip's geometry.
    #[must_use]
    pub fn empty_like(&self) -> Self {
        Self {
            layout: self.layout,
            header: self.header.clone(),
            frames: Vec::new(),
        }
    }
}

/// Statistics for one completed harness run.
///
/// `bytes_changed` is the anti-fabrication counter: it counts frame-data bytes
/// that differ between the demuxed input and the muxed output. An operation
/// that claims to alter the picture but reports `bytes_changed == 0` did
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClipStats {
    /// Number of frames written.
    pub frame_count: usize,
    /// Size of the input file in bytes.
    pub bytes_in: u64,
    /// Size of the written output file in bytes.
    pub bytes_out: u64,
    /// Frame-data bytes that differ between input and output.
    pub bytes_changed: u64,
}

// ---------------------------------------------------------------------------
// Y4M I/O
// ---------------------------------------------------------------------------

/// Derive the plane layout for a parsed Y4M header.
///
/// # Errors
///
/// Returns an error for chroma formats the harness cannot lay out.
pub fn layout_for_header(header: &Y4mHeader) -> Result<ChromaLayout> {
    ChromaLayout::for_chroma(header.chroma, header.width, header.height).ok_or_else(|| {
        anyhow::anyhow!(
            "Y4M chroma format '{}' is not supported by the frame harness",
            header.chroma
        )
    })
}

/// The shared "convert it first" error text for non-Y4M inputs.
///
/// `operation` names the CLI operation for the message (e.g.
/// `"timecode burn"`); every harness entry point funnels non-Y4M inputs
/// through here so the wording — and the conversion hint — is identical
/// wherever the contract is enforced.
#[must_use]
pub fn non_y4m_message(operation: &str, input: &Path) -> String {
    format!(
        "{operation} requires an uncompressed YUV4MPEG2 (.y4m) input, because it re-encodes \
         every frame. '{}' is not a Y4M file. Convert it first \
         (e.g. `oximedia transcode -i input.mp4 output.y4m`).",
        input.display()
    )
}

/// Read at most `buf.len()` bytes from the head of `path`.
fn read_prefix(path: &Path, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut filled = 0usize;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// How many leading bytes [`peek_y4m_header`] reads before falling back to a
/// full read. Y4M headers are a single line, so this is generous.
const HEADER_PEEK_BYTES: usize = 64 * 1024;

/// Parse only the stream header of a Y4M file, without decoding frames.
///
/// Reads a bounded prefix first and only falls back to a full read if the
/// header turns out to be longer than that prefix.
///
/// # Errors
///
/// Returns an error if the file is unreadable, is not Y4M, or declares a
/// chroma format the harness cannot lay out.
pub fn peek_y4m_header(operation: &str, input: &Path) -> Result<(Y4mHeader, ChromaLayout)> {
    let file_len = std::fs::metadata(input)
        .with_context(|| format!("Failed to stat input: {}", input.display()))?
        .len();

    let mut prefix = vec![
        0u8;
        HEADER_PEEK_BYTES
            .min(file_len as usize)
            .max(Y4M_MAGIC.len())
    ];
    let filled = read_prefix(input, &mut prefix)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;
    prefix.truncate(filled);

    if !prefix.starts_with(Y4M_MAGIC) {
        anyhow::bail!("{}", non_y4m_message(operation, input));
    }

    let header = match Y4mDemuxer::new(std::io::Cursor::new(prefix.as_slice())) {
        Ok(demuxer) => demuxer.header().clone(),
        Err(prefix_err) => {
            // The header may be longer than the prefix (a very long comment);
            // retry against the whole file before reporting a parse failure.
            if (filled as u64) >= file_len {
                return Err(anyhow::anyhow!(
                    "Failed to parse Y4M header of '{}': {prefix_err}",
                    input.display()
                ));
            }
            let raw = std::fs::read(input)
                .with_context(|| format!("Failed to read input: {}", input.display()))?;
            Y4mDemuxer::new(std::io::Cursor::new(raw.as_slice()))
                .map_err(|e| {
                    anyhow::anyhow!("Failed to parse Y4M header of '{}': {e}", input.display())
                })?
                .header()
                .clone()
        }
    };

    let layout = layout_for_header(&header)?;
    Ok((header, layout))
}

/// Decode a whole Y4M file into memory.
///
/// # Errors
///
/// Returns an error if the file is unreadable, is not Y4M, has an unsupported
/// chroma format, or contains a malformed frame.
pub fn read_y4m_clip(operation: &str, input: &Path) -> Result<PlanarClip> {
    let raw = std::fs::read(input)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;
    read_y4m_clip_from_bytes(operation, input, &raw)
}

/// Decode a Y4M byte buffer into memory.
///
/// `input` is used only for error messages.
///
/// # Errors
///
/// Returns an error if the buffer is not a parseable Y4M stream, has an
/// unsupported chroma format, or contains a malformed frame.
pub fn read_y4m_clip_from_bytes(operation: &str, input: &Path, raw: &[u8]) -> Result<PlanarClip> {
    if !raw.starts_with(Y4M_MAGIC) {
        anyhow::bail!("{}", non_y4m_message(operation, input));
    }

    let mut demuxer = Y4mDemuxer::new(std::io::Cursor::new(raw))
        .map_err(|e| anyhow::anyhow!("Failed to parse Y4M header of '{}': {e}", input.display()))?;
    let header = demuxer.header().clone();
    let frames_raw = demuxer
        .read_all_frames()
        .map_err(|e| anyhow::anyhow!("Failed to read Y4M frames of '{}': {e}", input.display()))?;

    if frames_raw.is_empty() {
        anyhow::bail!(
            "Y4M input '{}' contains no frames; nothing for {operation} to process.",
            input.display()
        );
    }

    PlanarClip::from_raw_frames(header, frames_raw)
}

/// Mux a clip into a Y4M byte stream.
///
/// # Errors
///
/// Returns an error if the muxer rejects the geometry or a frame.
pub fn encode_y4m_clip(clip: &PlanarClip) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut muxer = new_muxer(&clip.header, &mut out)?;
        for (i, frame) in clip.frames.iter().enumerate() {
            muxer
                .write_frame(&frame.data)
                .map_err(|e| anyhow::anyhow!("Failed to write Y4M frame {i}: {e}"))?;
        }
        muxer
            .finish()
            .map_err(|e| anyhow::anyhow!("Failed to finalise Y4M output: {e}"))?;
    }
    Ok(out)
}

/// Mux a clip and write it to `output` in a single filesystem write.
///
/// Returns the number of bytes written.
///
/// # Errors
///
/// Returns an error if muxing or the write fails. On failure no output file is
/// created.
pub fn write_y4m_clip(output: &Path, clip: &PlanarClip) -> Result<u64> {
    let bytes = encode_y4m_clip(clip)?;
    std::fs::write(output, &bytes)
        .with_context(|| format!("Failed to write output: {}", output.display()))?;
    Ok(bytes.len() as u64)
}

/// Build a Y4M muxer that reproduces `header`'s stream parameters.
fn new_muxer<'a>(
    header: &Y4mHeader,
    out: &'a mut Vec<u8>,
) -> Result<oximedia_container::mux::y4m::Y4mMuxer<&'a mut Vec<u8>>> {
    Y4mMuxerBuilder::new(header.width, header.height)
        .fps(header.fps_num.max(1), header.fps_den.max(1))
        .chroma(header.chroma)
        .interlace(header.interlace)
        .aspect_ratio(header.par_num, header.par_den)
        .build(out)
        .map_err(|e| anyhow::anyhow!("Failed to create Y4M muxer: {e}"))
}

/// Count differing bytes between two equal-length frame buffers.
fn diff_bytes(a: &[u8], b: &[u8]) -> u64 {
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count() as u64
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Whole-clip decode → process → encode.
///
/// The operation receives the entire decoded clip, so it can compute global
/// context (a motion trajectory, a histogram across the clip, …) before
/// producing the output clip. Use [`process_frames`] for streaming operations.
///
/// `operation` labels the caller in error messages.
///
/// # Errors
///
/// Returns an error if the input is not a usable Y4M clip, if `op` fails, if
/// the produced clip disagrees with the input geometry, or if muxing/writing
/// fails. No output file is written unless the whole pass succeeds.
pub fn process_clip<F>(operation: &str, input: &Path, output: &Path, op: F) -> Result<ClipStats>
where
    F: FnOnce(&PlanarClip) -> Result<PlanarClip>,
{
    let raw = std::fs::read(input)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;
    let bytes_in = raw.len() as u64;
    let clip = read_y4m_clip_from_bytes(operation, input, &raw)?;

    let processed = op(&clip)?;

    if processed.layout != clip.layout {
        anyhow::bail!(
            "{operation} changed the frame geometry ({}x{} → {}x{}); the harness contract is \
             geometry-preserving Y4M in / Y4M out",
            clip.layout.luma_w,
            clip.layout.luma_h,
            processed.layout.luma_w,
            processed.layout.luma_h
        );
    }

    let bytes_changed = clip
        .frames
        .iter()
        .zip(processed.frames.iter())
        .map(|(a, b)| diff_bytes(&a.data, &b.data))
        .sum();

    let bytes_out = write_y4m_clip(output, &processed)?;

    Ok(ClipStats {
        frame_count: processed.frames.len(),
        bytes_in,
        bytes_out,
        bytes_changed,
    })
}

/// Streaming decode → process → encode, one frame at a time.
///
/// The closure sees each frame's index and a mutable packed planar frame; it
/// must keep the frame's byte length (the Y4M frame size is fixed by the
/// header). Use [`process_clip`] when the operation needs the whole clip.
///
/// `operation` labels the caller in error messages.
///
/// # Errors
///
/// Returns an error if the input is not a usable Y4M clip, if `op` fails or
/// resizes a frame, or if muxing/writing fails. No output file is written
/// unless the whole pass succeeds.
pub fn process_frames<F>(
    operation: &str,
    input: &Path,
    output: &Path,
    mut op: F,
) -> Result<ClipStats>
where
    F: FnMut(usize, &mut PlanarFrame) -> Result<()>,
{
    let raw = std::fs::read(input)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;
    let bytes_in = raw.len() as u64;

    if !raw.starts_with(Y4M_MAGIC) {
        anyhow::bail!("{}", non_y4m_message(operation, input));
    }

    let mut demuxer = Y4mDemuxer::new(std::io::Cursor::new(raw.as_slice()))
        .map_err(|e| anyhow::anyhow!("Failed to parse Y4M header of '{}': {e}", input.display()))?;
    let header = demuxer.header().clone();
    let layout = layout_for_header(&header)?;
    let expected = layout.frame_size();

    let mut out_buf: Vec<u8> = Vec::with_capacity(raw.len());
    let mut frame_count = 0usize;
    let mut bytes_changed = 0u64;
    {
        let mut muxer = new_muxer(&header, &mut out_buf)?;

        loop {
            let Some(packed) = demuxer.read_frame().map_err(|e| {
                anyhow::anyhow!("Failed to read Y4M frame of '{}': {e}", input.display())
            })?
            else {
                break;
            };

            let original = packed.clone();
            let mut frame = PlanarFrame::new(layout, packed)
                .with_context(|| format!("Y4M frame {frame_count} has an unexpected size"))?;

            op(frame_count, &mut frame)
                .with_context(|| format!("{operation} failed on frame {frame_count}"))?;

            if frame.data.len() != expected {
                anyhow::bail!(
                    "{operation} resized frame {frame_count} from {expected} to {} bytes; the \
                     harness contract is geometry-preserving Y4M in / Y4M out",
                    frame.data.len()
                );
            }

            bytes_changed += diff_bytes(&original, &frame.data);
            muxer
                .write_frame(&frame.data)
                .map_err(|e| anyhow::anyhow!("Failed to write Y4M frame {frame_count}: {e}"))?;
            frame_count += 1;
        }

        if frame_count == 0 {
            anyhow::bail!(
                "Y4M input '{}' contains no frames; nothing for {operation} to process.",
                input.display()
            );
        }

        muxer
            .finish()
            .map_err(|e| anyhow::anyhow!("Failed to finalise Y4M output: {e}"))?;
    }

    std::fs::write(output, &out_buf)
        .with_context(|| format!("Failed to write output: {}", output.display()))?;

    Ok(ClipStats {
        frame_count,
        bytes_in,
        bytes_out: out_buf.len() as u64,
        bytes_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic Y4M clip (4:2:0) with a deterministic gradient.
    fn synthetic_y4m(width: u32, height: u32, frames: usize) -> Vec<u8> {
        let w = width as usize;
        let h = height as usize;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let mut data =
            format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C420jpeg\n").into_bytes();
        for t in 0..frames {
            data.extend_from_slice(b"FRAME\n");
            for y in 0..h {
                for x in 0..w {
                    data.push(((x * 3 + y * 5 + t * 7) % 256) as u8);
                }
            }
            for plane in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        data.push(((x * 2 + y + plane * 40 + t) % 256) as u8);
                    }
                }
            }
        }
        data
    }

    /// Scratch path for a fixture.
    ///
    /// The PID matters: this module is compiled into *both* the `oximedia`
    /// binary and the `oximedia-cli` lib target, so every test here runs twice
    /// in two concurrent processes. Fixed names would let one copy's cleanup
    /// delete the other copy's fixture mid-test.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_frame_harness_{}_{name}",
            std::process::id()
        ))
    }

    #[test]
    fn chroma_layout_plane_geometry() {
        let l420 = ChromaLayout::for_chroma(Y4mChroma::C420jpeg, 64, 48).expect("420 layout");
        assert_eq!(l420.frame_size(), 64 * 48 + 2 * 32 * 24);
        assert_eq!(l420.plane_count(), 3);
        assert_eq!(l420.plane_offset(0), Some(0));
        assert_eq!(l420.plane_offset(1), Some(64 * 48));
        assert_eq!(l420.plane_offset(2), Some(64 * 48 + 32 * 24));
        assert_eq!(l420.plane_offset(3), None);

        let l444a = ChromaLayout::for_chroma(Y4mChroma::C444alpha, 8, 8).expect("444alpha layout");
        assert_eq!(l444a.plane_count(), 4);
        assert_eq!(l444a.plane_offset(3), Some(8 * 8 * 3));
        assert_eq!(l444a.frame_size(), 8 * 8 * 4);

        let mono = ChromaLayout::for_chroma(Y4mChroma::Mono, 16, 16).expect("mono layout");
        assert_eq!(mono.plane_count(), 1);
        assert_eq!(mono.plane_offset(1), None);
    }

    #[test]
    fn read_write_round_trip_is_bit_exact() {
        let input = temp_path("rt_in.y4m");
        let output = temp_path("rt_out.y4m");
        std::fs::write(&input, synthetic_y4m(32, 16, 4)).expect("write fixture");

        let clip = read_y4m_clip("harness test", &input).expect("read clip");
        assert_eq!(clip.len(), 4);
        assert_eq!(clip.width(), 32);
        assert_eq!(clip.height(), 16);

        write_y4m_clip(&output, &clip).expect("write clip");
        let reread = read_y4m_clip("harness test", &output).expect("re-read clip");
        assert_eq!(reread.len(), clip.len());
        for (a, b) in clip.frames.iter().zip(reread.frames.iter()) {
            assert_eq!(a.data, b.data, "round trip must be bit-exact");
        }

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn process_frames_counts_changed_bytes() {
        let input = temp_path("pf_in.y4m");
        let output = temp_path("pf_out.y4m");
        std::fs::write(&input, synthetic_y4m(32, 16, 3)).expect("write fixture");

        let stats = process_frames("harness test", &input, &output, |_i, frame| {
            frame.luma_mut()[0] = frame.luma()[0].wrapping_add(17);
            Ok(())
        })
        .expect("process frames");

        assert_eq!(stats.frame_count, 3);
        assert_eq!(stats.bytes_changed, 3, "one byte per frame must change");
        assert!(stats.bytes_out > 0);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn process_clip_identity_changes_nothing() {
        let input = temp_path("pc_in.y4m");
        let output = temp_path("pc_out.y4m");
        std::fs::write(&input, synthetic_y4m(16, 16, 2)).expect("write fixture");

        let stats = process_clip("harness test", &input, &output, |clip| Ok(clip.clone()))
            .expect("process clip");
        assert_eq!(stats.frame_count, 2);
        assert_eq!(stats.bytes_changed, 0);
        assert_eq!(stats.bytes_in, stats.bytes_out);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn non_y4m_input_is_rejected_without_writing_output() {
        let input = temp_path("bad_in.mp4");
        let output = temp_path("bad_out.y4m");
        let _ = std::fs::remove_file(&output);
        std::fs::write(&input, b"\x00\x00\x00\x18ftypmp42not-a-y4m").expect("write fixture");

        let err = process_frames("harness test", &input, &output, |_, _| Ok(()))
            .expect_err("non-Y4M must error");
        let msg = format!("{err}");
        assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
        assert!(msg.contains("oximedia transcode"), "got: {msg}");
        assert!(!output.exists(), "no output may be fabricated");

        // The whole-clip entry point enforces the same contract.
        assert!(read_y4m_clip("harness test", &input).is_err());
        assert!(peek_y4m_header("harness test", &input).is_err());

        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn closure_failure_leaves_no_output() {
        let input = temp_path("fail_in.y4m");
        let output = temp_path("fail_out.y4m");
        let _ = std::fs::remove_file(&output);
        std::fs::write(&input, synthetic_y4m(16, 16, 2)).expect("write fixture");

        let err = process_frames("harness test", &input, &output, |_, _| {
            Err(anyhow::anyhow!("boom"))
        })
        .expect_err("closure failure must propagate");
        assert!(format!("{err}").contains("frame 0"), "got: {err}");
        assert!(!output.exists(), "no output may be fabricated");

        let _ = std::fs::remove_file(&input);
    }
}
