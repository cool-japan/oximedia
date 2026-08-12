# OxiMedia Python Bindings

**Status: [Stable]** | Version: 0.2.1 | Tests: extensively tested | Updated: 2026-08-12

Python bindings for OxiMedia, a royalty-free multimedia processing library written in Rust.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Video Codecs**: AV1 (encode/decode), VP9 (decode), VP8 (decode)
- **Audio Codecs**: Opus (encode/decode), Vorbis (decode), FLAC (decode)
- **Container Formats**: Matroska/WebM (demux/mux), Ogg (demux/mux), FLAC (demux), WAV (demux)
- **Filter Graph**: Scale, crop, volume, normalize filter bindings
- **Pipeline**: Multi-stage media pipeline builder
- **Batch Processing**: Batch transcoding operations
- **Progress Tracking**: Long-running operation progress tracking
- **Media Probing**: Media format and stream information queries
- **Metadata**: Typed metadata field access
- **Error Types**: Structured error types with categories and severity
- **Format Information**: Container capabilities and codec queries
- **Media Hashing**: Content hashing and fingerprinting
- `ml` — PyO3 bindings for `oximedia-ml` typed pipelines (SceneClassifier, ShotBoundaryDetector, AestheticScorer, ObjectDetector, FaceEmbedder)
- `neural` — PyO3 bindings for `oximedia-neural` tensor and inference models (Tensor, SceneClassifier, ThumbnailRanker, SrUpscaler, FeatureExtractor)
- `analytics` — PyO3 bindings for `oximedia-analytics` viewer-session tracking, engagement scoring, and attention heatmaps (ViewerSession, SessionMetrics, ContentEngagementScore)
- `cache` — PyO3 bindings for `oximedia-cache`'s LRU cache (LruCache, CacheStats)
- **Zero-copy operations** where possible
- **Thread-safe** Python bindings

## Installation

### From PyPI

```bash
pip install oximedia
```

### From Source

```bash
cd crates/oximedia-py
pip install maturin
maturin develop
```

## Quick Start

### Video Decoding

```python
import oximedia

# Create AV1 decoder
decoder = oximedia.Av1Decoder()

# Send compressed packet
decoder.send_packet(packet_data, pts=0)

# Receive decoded frame
frame = decoder.receive_frame()
if frame:
    print(f"Frame: {frame.width}x{frame.height}")
    y_plane = frame.plane_data(0)  # Y plane
    u_plane = frame.plane_data(1)  # U plane
    v_plane = frame.plane_data(2)  # V plane
```

### Video Encoding

```python
import oximedia

# Create encoder configuration
config = oximedia.EncoderConfig(
    width=1920,
    height=1080,
    framerate=(30, 1),
    crf=28.0,
    preset="medium",
    keyint=250
)

# Create AV1 encoder
encoder = oximedia.Av1Encoder(config)

# Encode frame
frame = oximedia.VideoFrame(1920, 1080, oximedia.PixelFormat("yuv420p"))
encoder.send_frame(frame)

# Receive encoded packet
packet = encoder.receive_packet()
if packet:
    print(f"Packet: {len(packet['data'])} bytes, keyframe={packet['keyframe']}")
```

### Audio Decoding

```python
import oximedia

# Create Opus decoder
decoder = oximedia.OpusDecoder(sample_rate=48000, channels=2)

# Decode packet
audio_frame = decoder.decode_packet(packet_data)

print(f"Audio: {audio_frame.sample_count} samples, {audio_frame.channels} channels")

# Get samples as float32
samples_f32 = audio_frame.to_f32()

# Get samples as int16
samples_i16 = audio_frame.to_i16()
```

### Container Demuxing

```python
import oximedia

# Open Matroska file
demuxer = oximedia.MatroskaDemuxer("video.mkv")
demuxer.probe()

# Get stream information
for stream in demuxer.streams():
    print(f"Stream {stream.index}: {stream.codec}")
    if stream.width:
        print(f"  Video: {stream.width}x{stream.height}")
    if stream.sample_rate:
        print(f"  Audio: {stream.sample_rate}Hz, {stream.channels} channels")

# Read packets
while True:
    try:
        packet = demuxer.read_packet()
        print(f"Packet: stream={packet.stream_index}, size={packet.size()}, "
              f"pts={packet.pts}, keyframe={packet.is_keyframe()}")
    except StopIteration:
        break
```

### Container Muxing

```python
import oximedia

# Create muxer
muxer = oximedia.MatroskaMuxer("output.mkv", title="My Video")

# Write header
muxer.write_header()

# Write packets
for packet in packets:
    muxer.write_packet(packet)

# Finalize
muxer.write_trailer()
```

### Zero-Copy NumPy Access

`VideoFrame.plane(i)`, `AudioFrame.samples`, and `cv2_compat.Mat` implement
the PyO3 buffer protocol, so NumPy can wrap the underlying Rust buffer
without copying:

```python
import oximedia
import numpy as np

# Decode a video file
decoder = oximedia.Av1Decoder()
# ... feed packets, get frames ...
frame = ...  # an oximedia.VideoFrame returned by the decoder

# Zero-copy numpy view of plane 0 (Y in YUV420P)
plane0 = np.asarray(frame.plane(0))
print(plane0.shape, plane0.dtype)  # (height, width, 1) uint8

# Audio interleaved samples as a numpy view
audio = oximedia.AudioFrame(...)
samples = np.asarray(audio.samples)
```

Each call to `plane(i)` / `samples` returns its own buffer view, so multiple
concurrent views of the same frame are safe.

## Command-Line Interface

The package ships a `python -m oximedia` entry point (installed with the
wheel — no separate console script needed):

```bash
# Probe a media file (text or JSON output)
python -m oximedia probe input.mkv
python -m oximedia probe input.mkv --json

# Transcode using a built-in preset
python -m oximedia transcode input.mkv output.webm --preset youtube-1080p
python -m oximedia transcode input.mkv output.webm --crf 28

# Quality assessment between two files
python -m oximedia quality reference.mp4 distorted.mp4 --metric all

# cv2-style colour conversion / format conversion
python -m oximedia cv2 cvt-color input.png output.png --code BGR2RGB
python -m oximedia cv2 convert input.bmp output.jpg

# Inspect available presets / codecs / version
python -m oximedia presets
python -m oximedia codecs
python -m oximedia version
```

Run `python -m oximedia --help` (or `python -m oximedia <subcommand> --help`)
for the full argument list.

## Supported Codecs

OxiMedia only supports royalty-free, patent-unencumbered codecs:

### Video
- **AV1** - Alliance for Open Media codec (encode + decode)
- **VP9** - Google's royalty-free codec (decode)
- **VP8** - Google's earlier royalty-free codec (decode)

### Audio
- **Opus** - Modern low-latency audio codec (encode + decode)
- **Vorbis** - Xiph.Org audio codec (decode)
- **FLAC** - Lossless audio codec (decode)

### Containers
- **Matroska/WebM** (.mkv, .webm) — demux + mux
- **Ogg** (.ogg, .opus, .oga) — demux + mux
- **FLAC** (.flac) — demux
- **WAV** (.wav) — demux

## API Overview

**Core Python classes:**
- `PixelFormat`, `SampleFormat`, `ChannelLayout` — Format descriptors
- `VideoFrame`, `AudioFrame` — Frame data containers
- `EncoderConfig`, `EncoderPreset`, `Rational` — Encoding configuration

**Video codecs:**
- `Av1Decoder`, `Av1Encoder` — AV1 codec
- `Vp9Decoder`, `Vp8Decoder` — VP9/VP8 decoders

**Audio codecs:**
- `OpusDecoder`, `OpusEncoder`, `OpusEncoderConfig` — Opus codec
- `VorbisDecoder`, `FlacDecoder` — Vorbis/FLAC decoders

**Filters:**
- `PyScaleConfig`, `PyCropConfig`, `PyVolumeConfig`, `PyNormalizeConfig` — Filter configurations

**Container:**
- `Packet`, `StreamInfo` — Packet and stream data
- `MatroskaDemuxer`, `OggDemuxer` — Container demuxers
- `MatroskaMuxer`, `OggMuxer` — Container muxers

**Probe/info:**
- `PyVideoInfo`, `PyAudioInfo`, `PyStreamInfo`, `PyMediaInfo` — Media information

**Advanced (public modules):**
- `batch`, `batch_bindings` — Batch processing
- `codec_info` — Codec information queries
- `error_types` — Structured error types
- `filter_bindings` — Filter graph bindings
- `format_info` — Format information
- `media_hash` — Content hashing/fingerprinting
- `pipeline_bindings`, `pipeline_builder` — Pipeline construction
- `progress_tracker` — Progress tracking
- `py_config` — Configuration builder
- `py_error` — Error handling
- `py_metadata` — Metadata access
- `stream_reader` — Streaming reader utilities
- `timeline` — Timeline management
- `transcode_options` — Transcoding options
- `video_bindings`, `video_meta` — Video utilities

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

## Patent Protection

OxiMedia is designed to only work with royalty-free codecs. Attempting to use patent-encumbered codecs (H.264, H.265, AAC, etc.) will result in an error.
