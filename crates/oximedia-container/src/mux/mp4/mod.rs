//! MP4/ISOBMFF muxer.
//!
//! Implements progressive and fragmented MP4 (fMP4) muxing for patent-free
//! codecs (AV1, VP9). Produces valid ISOBMFF files with proper `ftyp`, `moov`,
//! and `mdat` atom layout.
//!
//! # Box Layout (Progressive MP4)
//!
//! ```text
//! [ftyp][moov[mvhd][trak[tkhd][mdia[mdhd][hdlr][minf[vmhd/smhd][dinf[dref]][stbl[stsd][stts][stsc][stsz][stco]]]]]][mdat]
//! ```
//!
//! # Box Layout (Fragmented MP4)
//!
//! ```text
//! [ftyp][moov[mvhd][mvex[trex]][trak(empty stbl)...]]
//!   ([sidx][moof[mfhd][traf[tfhd][tfdt][trun]]][mdat])...
//! ```
//!
//! # Example
//!
//! ```ignore
//! use oximedia_container::mux::mp4::{Mp4Muxer, Mp4Config, Mp4FragmentMode};
//! use oximedia_container::{StreamInfo, Packet};
//!
//! let config = Mp4Config::new().with_mode(Mp4FragmentMode::Progressive);
//! let mut muxer = Mp4Muxer::new(config);
//!
//! // Add streams, write packets, finalize
//! muxer.add_stream(video_stream)?;
//! muxer.write_header()?;
//! muxer.write_packet(&packet)?;
//! let data = muxer.finalize()?;
//! ```

#![forbid(unsafe_code)]

mod av1c;
mod basic;
pub mod esds;
pub mod simple;
mod writer;

pub use basic::{BasicMp4Error, BasicMp4Muxer};
pub use esds::{build_audio_specific_config, build_esds_box, build_esds_payload, EsdsParams};
pub use simple::{
    AudioCodecInfo, FourCC, Mp4Sample, Mp4TrackWriter, SimpleMp4Config, SimpleMp4Error,
    SimpleMp4Muxer, TrackCodec, VideoCodecInfo,
};
pub use writer::{
    build_sidx, Mp4Config, Mp4FragmentMode, Mp4Mode, Mp4Muxer, Mp4SampleEntry, Mp4TrackState,
};
