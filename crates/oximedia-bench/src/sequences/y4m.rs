//! Y4M (YUV4MPEG2) streaming reader.
//!
//! Split out of [`super`] to keep that module under the 2000-line limit.

/// Errors produced during Y4M parsing.
#[derive(Debug, thiserror::Error)]
pub enum Y4mError {
    /// I/O error from the underlying reader.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The file does not start with the `YUV4MPEG2` magic string.
    #[error("missing YUV4MPEG2 magic bytes in stream header")]
    MissingMagic,
    /// A required field (W or H) is missing or zero.
    #[error("invalid or missing field in Y4M header: {field}")]
    MissingField {
        /// The name of the missing header field (e.g. `"W"` or `"H"`).
        field: &'static str,
    },
    /// Frame separator `FRAME` not found where expected.
    #[error("expected FRAME separator, got: {got:?}")]
    MissingFrameSeparator {
        /// The bytes that were found instead of `FRAME`.
        got: String,
    },
    /// The frame plane data is shorter than the declared dimensions.
    #[error("frame payload truncated: expected {expected} bytes, got {got}")]
    TruncatedFrame {
        /// Expected byte count for the full frame.
        expected: usize,
        /// Actual byte count read.
        got: usize,
    },
    /// Malformed number in header field.
    #[error("cannot parse integer in field {field}: {source}")]
    ParseInt {
        /// The header field name that failed to parse.
        field: &'static str,
        /// The underlying parse error.
        #[source]
        source: std::num::ParseIntError,
    },
}

/// A single decoded Y4M frame with presentation timestamp and planar data.
#[derive(Debug, Clone)]
pub struct Y4mFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame index (0-based).
    pub pts: u64,
    /// Luma (Y) plane — `width × height` bytes.
    pub plane_y: Vec<u8>,
    /// Cb (U) chroma plane.
    pub plane_u: Vec<u8>,
    /// Cr (V) chroma plane.
    pub plane_v: Vec<u8>,
    /// Chroma subsampling descriptor, e.g. `"420"`, `"444"`, `"422"`.
    pub chroma: String,
}

/// Streaming iterator over frames in a YUV4MPEG2 (Y4M) file.
///
/// Conforms to the Xiph.org YUV4MPEG2 specification:
///   - Header: `YUV4MPEG2 W<w> H<h> F<num>:<den> I<p|t|b|m> A<par_w>:<par_h> C<chroma>\n`
///   - Frame:  `FRAME\n` followed by planar pixel data
///
/// Optional field defaults:
/// - F → 25:1 (frame rate)
/// - I → p (progressive)
/// - A → 1:1 (pixel aspect ratio)
/// - C → 420
///
/// Call [`Y4mSequence::open`] to open a file or [`Y4mSequence::from_reader`] to
/// wrap any `Read` implementor.
pub struct Y4mSequence<R: std::io::Read> {
    reader: R,
    width: u32,
    height: u32,
    chroma: String,
    frame_index: u64,
    exhausted: bool,
}

impl Y4mSequence<std::io::BufReader<std::fs::File>> {
    /// Open a Y4M file at `path` and parse the stream header.
    pub fn open(path: &std::path::Path) -> Result<Self, Y4mError> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(std::io::BufReader::new(file))
    }
}

impl<R: std::io::Read> Y4mSequence<R> {
    /// Parse the Y4M stream header from an arbitrary reader.
    pub fn from_reader(mut reader: R) -> Result<Self, Y4mError> {
        let header = read_until_newline(&mut reader)?;
        Self::parse_header(header, reader)
    }

    /// Width declared in the stream header.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height declared in the stream header.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Chroma subsampling descriptor declared in the stream header.
    #[must_use]
    pub fn chroma(&self) -> &str {
        &self.chroma
    }

    /// Decode the next frame from the stream.
    ///
    /// Returns `Ok(None)` at end-of-stream.
    pub fn next_frame(&mut self) -> Result<Option<Y4mFrame>, Y4mError> {
        if self.exhausted {
            return Ok(None);
        }

        // Read the FRAME\n separator (or detect EOF).
        let separator = match read_until_newline(&mut self.reader) {
            Ok(s) => s,
            Err(Y4mError::Io(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.exhausted = true;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        if separator.is_empty() {
            // Empty line at end of file
            self.exhausted = true;
            return Ok(None);
        }

        // The separator must start with "FRAME" (optional per-frame tags follow).
        if !separator.starts_with("FRAME") {
            return Err(Y4mError::MissingFrameSeparator { got: separator });
        }

        // Compute plane sizes based on chroma subsampling.
        let (y_size, u_size, v_size) = plane_sizes(self.width, self.height, &self.chroma);
        let total = y_size + u_size + v_size;

        let mut buf = vec![0u8; total];
        let mut filled = 0;
        while filled < total {
            match self.reader.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => return Err(Y4mError::Io(e)),
            }
        }
        if filled < total {
            self.exhausted = true;
            if filled == 0 {
                return Ok(None);
            }
            return Err(Y4mError::TruncatedFrame {
                expected: total,
                got: filled,
            });
        }

        let pts = self.frame_index;
        self.frame_index += 1;

        let plane_y = buf[..y_size].to_vec();
        let plane_u = buf[y_size..y_size + u_size].to_vec();
        let plane_v = buf[y_size + u_size..].to_vec();

        Ok(Some(Y4mFrame {
            width: self.width,
            height: self.height,
            pts,
            plane_y,
            plane_u,
            plane_v,
            chroma: self.chroma.clone(),
        }))
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn parse_header(header: String, reader: R) -> Result<Self, Y4mError> {
        if !header.starts_with("YUV4MPEG2") {
            return Err(Y4mError::MissingMagic);
        }

        let mut width: Option<u32> = None;
        let mut height: Option<u32> = None;
        let mut chroma = "420".to_string();

        // Tokens are space-separated after the magic.
        for token in header["YUV4MPEG2".len()..].split_whitespace() {
            let (tag, val) = token.split_at(1);
            match tag {
                "W" => {
                    width = Some(val.parse::<u32>().map_err(|e| Y4mError::ParseInt {
                        field: "W",
                        source: e,
                    })?);
                }
                "H" => {
                    height = Some(val.parse::<u32>().map_err(|e| Y4mError::ParseInt {
                        field: "H",
                        source: e,
                    })?);
                }
                "C" => {
                    chroma = val.to_string();
                }
                // F, I, A — optional, parsed but not stored for now
                _ => {}
            }
        }

        let width = width.ok_or(Y4mError::MissingField { field: "W" })?;
        let height = height.ok_or(Y4mError::MissingField { field: "H" })?;

        if width == 0 {
            return Err(Y4mError::MissingField { field: "W" });
        }
        if height == 0 {
            return Err(Y4mError::MissingField { field: "H" });
        }

        Ok(Self {
            reader,
            width,
            height,
            chroma,
            frame_index: 0,
            exhausted: false,
        })
    }
}

// ─── Y4mReader / Y4mWriter — re-exported from the y4m_rw submodule ───────────

pub use crate::y4m_rw::{BenchFrame, Y4mColorFormat, Y4mReader, Y4mWriter};

/// Read bytes from `reader` until a `\n` byte, returning the consumed string
/// (without the newline).  Returns `Y4mError::Io(UnexpectedEof)` if the reader
/// is exhausted before a newline is found.
fn read_until_newline<R: std::io::Read>(reader: &mut R) -> Result<String, Y4mError> {
    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                if buf.is_empty() {
                    // True EOF — signal with UnexpectedEof so callers can distinguish.
                    return Err(Y4mError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "EOF before newline",
                    )));
                }
                // Partial line without terminator (treat as last line).
                break;
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                buf.push(byte[0]);
            }
            Err(e) => return Err(Y4mError::Io(e)),
        }
    }
    String::from_utf8(buf).map_err(|_| Y4mError::MissingMagic) // reuse as "not ASCII"
}

/// Compute (Y, U, V) plane byte sizes for the given chroma subsampling string.
fn plane_sizes(width: u32, height: u32, chroma: &str) -> (usize, usize, usize) {
    let w = width as usize;
    let h = height as usize;
    let y = w * h;

    match chroma {
        "444" => (y, y, y),
        "422" => (y, (w / 2) * h, (w / 2) * h),
        "411" => (y, (w / 4) * h, (w / 4) * h),
        "440" => (y, w * (h / 2), w * (h / 2)),
        "mono" | "400" => (y, 0, 0),
        // Default: 420 (4:2:0)
        _ => (y, (w / 2) * (h / 2), (w / 2) * (h / 2)),
    }
}
