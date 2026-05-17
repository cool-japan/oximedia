# Wire Formats, Protocols, and FFI Conventions

> **Scope.** This doc covers the layer *underneath* codec internals: how
> compressed bytes are framed, packetized, authenticated, and handed
> across the FFI boundary into Apple media frameworks. It does **not**
> cover how a video codec actually compresses pixels — for that, see
> [`codec_internals.md`](codec_internals.md).

Each section is a self-contained reference — the diagrams are the same
ones used when implementing the relevant code, and the cross-references
point at the in-tree implementation.

## H.264 NAL units and start-code framing

H.264 is a stream of NAL (Network Abstraction Layer) units. The first byte
of every NAL unit packs three fields:

```text
 bit:  7   6 5   4 3 2 1 0
      ┌─┬─────┬─────────┐
      │F│ NRI │ nal_type│
      └─┴─────┴─────────┘
       │   │      │
       │   │      └─ NAL unit type (5 bits, 0–31)
       │   └──────── nal_ref_idc (2 bits, 0 = disposable, 3 = reference)
       └──────────── forbidden_zero_bit (always 0)
```

Common types:

|Type|Name|Meaning|
|---|---|---|
|1|non-IDR slice|inter-coded picture|
|5|IDR slice|instantaneous decoder refresh (keyframe)|
|7|SPS|sequence parameter set (resolution, profile, level)|
|8|PPS|picture parameter set (entropy mode, slice options)|
|6|SEI|supplemental enhancement information (timing, HDR metadata)|

NAL units are *delimited* one of two ways:

### Annex-B (network / RTSP / RTP convention)

NAL units are concatenated with **start codes** between them:

```text
 00 00 01    or    00 00 00 01    ← start code
 <NAL byte 0> <NAL byte 1> …      ← NAL unit payload
 00 00 01                          ← next start code
 …
```

A decoder scans for `00 00 01` or `00 00 00 01` to find boundaries. The
4-byte form is used when the next NAL is "primary" (SPS/PPS/IDR); the
3-byte form is the default. To prevent the byte sequence `00 00 01` from
appearing inside a payload by accident, encoders insert an "emulation
prevention byte" (`0x03`) when `00 00 00`, `00 00 01`, `00 00 02`, or
`00 00 03` would otherwise occur — decoders strip these.

### AVCC (ISOBMFF / MP4 / VideoToolbox convention)

No start codes. Each NAL unit is prefixed by a 32-bit big-endian length
field:

```text
 ┌─────────────┬───────────────┐
 │ 32-bit BE   │  NAL unit     │
 │ length = N  │  N bytes      │
 └─────────────┴───────────────┘
 ┌─────────────┬───────────────┐
 │ 32-bit BE   │  NAL unit     │
 │ length = M  │  M bytes      │
 └─────────────┴───────────────┘
```

Parameter sets (SPS, PPS) are *not* in the bytestream in AVCC — they're
configured separately at decoder-create time. VideoToolbox takes them via
`CMVideoFormatDescriptionCreateFromH264ParameterSets`. MP4 stores them in
the `avcC` box at sample-entry level.

### Conversion

Going from Annex-B to AVCC requires (a) splitting the bytestream at start
codes, (b) dropping the SPS/PPS NALs (type 7 / 8), (c) emitting a 4-byte
length prefix before each remaining NAL. Going the other way: emit
`00 00 00 01` between length-prefixed payloads.

**Implementation:** [crates/oximedia-vtb/src/nal.rs](../crates/oximedia-vtb/src/nal.rs) — `AnnexBIter`,
`extract_sps_pps`, `annex_b_to_avcc`, `avcc_to_annex_b`.

## RTP packet anatomy (RFC 3550 §5.1)

Every RTP packet starts with a 12-byte fixed header:

```text
  0                   1                   2                   3
  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
 |V=2|P|X|  CC   |M|     PT      |       sequence number         |
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
 |                           timestamp                           |
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
 |           synchronization source (SSRC) identifier            |
 +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
 |            contributing source (CSRC) identifiers             |
 |                          (CC × 32-bit)                        |
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
 │ if X=1: extension profile (16) | extension length (16, words) │
 │                         extension data                        │
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
 │                            payload                            │
 │                                                               │
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
 │  optional padding…              │     padding length (8 bit)  │ ← only if P=1
 +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

|Field|Bits|Meaning|
|---|---|---|
|V|2|version, always 2|
|P|1|padding present at end of packet|
|X|1|extension header present|
|CC|4|CSRC count (0–15)|
|M|1|marker bit, codec-specific (often: end of frame)|
|PT|7|payload type — 0–95 static map (RFC 3551), 96–127 dynamic (from SDP `a=rtpmap`)|
|sequence|16|per-packet counter, wraps every ~65k packets|
|timestamp|32|media clock; for H.264 normally 90 kHz|
|SSRC|32|sender identity, picked once per session|

Payload start offset = `12 + (CC × 4) + (X ? 4 + ext_len_words × 4 : 0)`.
Payload end = `len - (P ? buf[len-1] : 0)`.

### Sequence-number wrap math

16-bit sequence numbers wrap roughly every minute on a busy video stream,
so a raw subtraction `new - prev` is wrong across the boundary. The
correct *signed* delta modulo 2^16:

```rust
let diff = new.wrapping_sub(prev) as i32; // 0..=65_535
if diff > 32_768 { diff - 65_536 } else { diff }
```

That treats `(65_535, 0) → +1`, `(0, 65_535) → -1`. From the signed delta:

- `delta == 0` → duplicate packet
- `delta < 0` → reordered packet (arrived earlier than the highest seen)
- `delta > 1` → gap of `(delta - 1)` packets

**Implementation:** [crates/oximedia-net/src/rtsp/rtp.rs](../crates/oximedia-net/src/rtsp/rtp.rs) — `RtpPacket::parse`,
`SequenceTracker::observe`, `signed_seq_delta`.

## TCP-interleaved transport framing (RFC 2326 §10.12)

When RTSP requests `Transport: RTP/AVP/TCP;interleaved=N-N+1`, RTP and
RTCP packets are framed inline on the same TCP connection that carries
RTSP requests/responses:

```text
 +------+---------+--------+-----------+
 | 0x24 | channel | length |   data    |
 +------+---------+--------+-----------+
   1 B    1 B       2 B BE   length B
```

`0x24` is the ASCII `$` character. The trick: RTSP messages always start
with `RTSP/1.0` (response) or a method name (request) — never `$` — so a
single peek at the next byte distinguishes the two framings. Demuxing is
a state-free byte-test.

**Implementation:** [crates/oximedia-net/src/rtsp/transport.rs](../crates/oximedia-net/src/rtsp/transport.rs) — `next_frame`,
`encode_interleaved`.

## SDP (Session Description Protocol, RFC 8866)

Line-oriented `<type>=<value>` records, each `\r\n`-terminated:

```text
v=0                                       ← version
o=- 0 0 IN IP4 0.0.0.0                    ← origin (username sessid version nettype addrtype addr)
s=My Camera                               ← session name
c=IN IP4 0.0.0.0                          ← session-level connection
t=0 0                                     ← timing (start, stop)
a=control:*                               ← aggregate-control URI
m=video 0 RTP/AVP 96                      ← media: type port transport payload-types
a=rtpmap:96 H264/90000                    ← codec name + clock-rate for dynamic PT
a=fmtp:96 packetization-mode=1; profile-level-id=42E01F; sprop-parameter-sets=…
                                          ← codec-specific format parameters
a=control:trackID=1                       ← per-stream control URI
m=audio 0 RTP/AVP 0                       ← another stream
a=rtpmap:0 PCMU/8000
```

Key fields for an RTSP client:

|Line|Used for|
|---|---|
|`m=`|Media type + transport + dynamic PT list|
|`a=rtpmap:PT codec/clock-rate[/channels]`|Map dynamic PT → codec name|
|`a=fmtp:PT params`|Per-codec config (SPS/PPS for H.264 via `sprop-parameter-sets`)|
|`a=control:URI`|What URL to SETUP for this track (absolute or relative to base)|

Most other lines (`t=`, `i=`, `u=`, `e=`, `p=`, `b=`, `r=`, `z=`, `k=`)
are not needed for playback.

**Implementation:** [crates/oximedia-net/src/rtsp/sdp.rs](../crates/oximedia-net/src/rtsp/sdp.rs) — `SessionDescription::parse`,
`MediaDescription::primary_rtpmap`, `primary_fmtp`.

## HTTP Digest authentication (RFC 2617, used by RTSP)

Challenge/response auth that never sends the plaintext password. Camera
sends a 401 with `WWW-Authenticate: Digest realm=…, nonce=…, qop=…`;
client computes a hash and retries with `Authorization: Digest …`.

### The math

```text
HA1  = MD5( username : realm : password )
HA2  = MD5( method   : uri )

response = MD5( HA1 : nonce : nc : cnonce : qop : HA2 )    ← when qop=auth
response = MD5( HA1 : nonce : HA2 )                         ← RFC 2069 fallback (no qop)
```

|Field|Source|Notes|
|---|---|---|
|`realm`|server|realm name from the 401|
|`nonce`|server|one-time server challenge|
|`nc`|client|nonce-count, 8 hex digits, increments per request reusing the same nonce|
|`cnonce`|client|client nonce, arbitrary unique string (we use `seed-hex + timestamp-hex`)|
|`qop`|server|`auth` (request integrity) or `auth-int` (request+body integrity); we support `auth`|
|`method`|request|uppercase RTSP method name|
|`uri`|request|exact request-URI being authenticated|

The cnonce + nc + nonce triple lets the server detect replay: re-using a
`(nonce, nc)` pair is a protocol violation.

### MD5 (RFC 1321)

128-bit Merkle-Damgård hash. Initialize four 32-bit words `(A0, B0, C0,
D0) = (0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476)`. Pad input to a
multiple of 64 bytes: append `0x80`, then zeros until `len ≡ 56 (mod 64)`,
then 8-byte little-endian message length in bits. For each 64-byte chunk,
copy `(A, B, C, D)` to `(a, b, c, d)`, run 64 rounds:

```text
for i in 0..64:
    f, g = match i:
        0..=15  => ( (b & c) | (¬b & d), i )                 ← F round
        16..=31 => ( (d & b) | (¬d & c), (5·i + 1) mod 16 )  ← G round
        32..=47 => ( b ⊕ c ⊕ d,          (3·i + 5) mod 16 )  ← H round
        48..=63 => ( c ⊕ (b | ¬d),       (7·i)     mod 16 )  ← I round
    a = b + rotate_left( a + f + K[i] + M[g], S[i] )
    rotate (a, b, c, d) ← (d, a, b, c)

(A0, B0, C0, D0) += (a, b, c, d)
```

After all chunks, output is `A0 || B0 || C0 || D0` in little-endian byte
order. `K[64]` and `S[64]` are fixed constants from the RFC.

MD5 is cryptographically broken (collisions are findable) but is still the
wire-level requirement for RFC 2617 — replace it only if RFC 7616 (SHA-256
Digest) is the target.

**Implementation:** [crates/oximedia-net/src/rtsp/auth.rs](../crates/oximedia-net/src/rtsp/auth.rs) — `Challenge::parse`,
`Challenge::build_authorization`, inline `md5()`.

## NV12 pixel format (kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)

VideoToolbox's default H.264 decode output. **Bi-planar 4:2:0**:

```text
 Plane 0 (Y, luma):                Plane 1 (CbCr, chroma):
 ┌───────────────────────┐         ┌───────────────────────┐
 │ Y00 Y01 Y02 … Y0w     │         │ Cb00 Cr00 Cb01 Cr01 … │
 │ Y10 Y11 Y12 …         │         │ Cb10 Cr10 …           │
 │  …                    │         │  …                    │
 │  h rows               │         │  h/2 rows             │
 │  w bytes each         │         │  w bytes each         │
 └───────────────────────┘         └───────────────────────┘
```

- Luma plane: `w × h` bytes, one byte per pixel.
- Chroma plane: `w × (h/2)` bytes — but the *samples* are arranged as
  `(w/2) × (h/2)` 2-byte CbCr pairs interleaved. The byte width is still
  `w`. Each CbCr sample covers a 2×2 block of luma pixels.
- "Video range": Y is 16–235, Cb/Cr are 16–240. "Full range" uses 0–255.

**FourCC codes:**

|Format|FourCC|Hex|
|---|---|---|
|4:2:0 bi-planar video range|`'420v'`|`0x3432_3076`|
|4:2:0 bi-planar full range|`'420f'`|`0x3432_3066`|
|4:2:0 planar (Y/U/V three planes)|`'y420'`|`0x7934_3230`|

### Stride and plane access

CVPixelBuffer rows are aligned (typically to 16 or 64 bytes), so the
stride is ≥ width:

```text
GetBytesPerRowOfPlane(buf, p)   ← actual stride in bytes
GetWidthOfPlane(buf, p)         ← logical width (bytes for Y, 2-byte pairs for UV)
GetHeightOfPlane(buf, p)        ← row count
GetBaseAddressOfPlane(buf, p)   ← first byte of plane
```

To copy out plane data row by row:

```rust
for row in 0..height_p {
    let src = base_ptr + row * stride_p;
    let dst = our_buf  + row * our_stride;
    memcpy(dst, src, row_bytes);
}
```

The lock/unlock pair is mandatory:
`CVPixelBufferLockBaseAddress(buf, kCVPixelBufferLock_ReadOnly = 1)` →
read → `CVPixelBufferUnlockBaseAddress(buf, kCVPixelBufferLock_ReadOnly)`.

**Implementation:** [crates/oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs) — `extract_video_frame`.

## CoreFoundation refcounting

Every Apple Core* framework (CoreFoundation, CoreMedia, CoreVideo,
VideoToolbox, AudioToolbox) builds on `CFType`. Two rules:

1. APIs whose name contains `Create` or `Copy` return a `+1`-retained
   reference — the caller owns one retain and must `CFRelease` to balance.
2. APIs whose name contains `Get` return a **borrowed** reference — the
   caller does *not* own a retain and must `CFRetain` first if they want
   to outlive the call.

`CFRetain` and `CFRelease` are atomic and thread-safe; the underlying
object may not be, but the reference count itself is.

### RAII pattern in Rust

```rust
pub struct CfOwned<T> { ptr: NonNull<T>, _marker: PhantomData<T> }

impl<T> CfOwned<T> {
    /// Adopt a +1-retained pointer (from a Create/Copy API).
    pub unsafe fn from_create(ptr: *mut T) -> Option<Self> { /* … */ }

    /// Take shared ownership of a borrowed pointer (from a Get API).
    /// Bumps the retain count.
    pub unsafe fn from_get(ptr: *mut T) -> Option<Self> { /* CFRetain + … */ }

    pub fn as_ptr(&self) -> *mut T { /* … */ }
}

impl<T> Clone for CfOwned<T> {
    fn clone(&self) -> Self { /* CFRetain + new wrapper */ }
}

impl<T> Drop for CfOwned<T> {
    fn drop(&mut self) { /* CFRelease */ }
}
```

The `Clone` impl retains so both copies stay valid; `Drop` releases
exactly the retain the wrapper owns. The invariant "every `CfOwned`
corresponds to exactly one `+1` retain" makes leaks and double-frees
type-system errors instead of runtime errors.

**Implementation:** [crates/oximedia-vtb/src/cf.rs](../crates/oximedia-vtb/src/cf.rs).

## CMTime structure

CoreMedia's timestamp type. Rational number + flags:

```c
typedef struct {
    int64_t  value;      // numerator (signed)
    int32_t  timescale;  // denominator (unsigned in practice)
    uint32_t flags;      // bit 0 = valid, bit 1 = "positive infinity", etc.
    int64_t  epoch;      // for wall-clock-correlated time; usually 0
} CMTime;
```

Effective time = `value / timescale` seconds, when `flags & 1 == 1` (valid).

For H.264 the conventional timescale is **90 000** (90 kHz) — matches the
RTP timestamp clock for video, so a PTS can be passed through both layers
without re-scaling. A 30 fps frame is 3000 ticks long; 25 fps is 3600.

|Common construction|`value`|`timescale`|`flags`|
|---|---|---|---|
|Valid PTS = 1500 ticks at 90 kHz|1500|90 000|1|
|Invalid / unspecified|0|0|0|
|Positive infinity|0|0|1 \|(1<<2)|

`CMSampleTimingInfo` bundles three CMTimes: `duration`,
`presentationTimeStamp`, `decodeTimeStamp` — passed to
`CMSampleBufferCreateReady` to attach timing to a sample.

**Implementation:** [crates/oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs) — `create_sample_buffer`,
`invalid_cm_time`.

## VideoToolbox decompression-session callback model

`VTDecompressionSessionCreate` takes a callback record:

```c
typedef struct {
    void (*decompressionOutputCallback)(
        void *decompressionOutputRefCon,    // opaque user pointer
        void *sourceFrameRefCon,            // opaque per-frame pointer
        OSStatus status,                    // 0 on success
        VTDecodeInfoFlags infoFlags,
        CVImageBufferRef imageBuffer,       // the decoded frame
        CMTime presentationTimeStamp,
        CMTime presentationDuration);
    void *decompressionOutputRefCon;
} VTDecompressionOutputCallbackRecord;
```

The callback is invoked once per decoded frame, on VT's internal serial
dispatch queue (a thread VT manages itself — *not* the thread that called
`DecodeFrame`).

### Bridging C callback → Rust state

C function pointers can't capture state, so the standard pattern is:

```text
                  ┌─────────────────────────┐
   user data ───→ │  Box<CallbackContext>   │ ← Rust state (queue, etc.)
   (refcon)       └─────────────────────────┘
                              ↑
                              │ trampoline casts back
                              │
   ┌───────────────────────────────────────────┐
   │ extern "C" fn output_callback_trampoline( │
   │     refcon: *mut c_void, …) {             │
   │   let ctx = &*(refcon as *mut CallbackContext);
   │   /* push frame into ctx.queue */
   │ }                                         │
   └───────────────────────────────────────────┘
```

Lifecycle:

1. Allocate `CallbackContext` in a `Box`; `Box::into_raw` to get a stable
   pointer.
2. Pass that pointer as the session's `refcon`.
3. The session holds the pointer for its entire lifetime.
4. On `Drop`:
   - **First** call `VTDecompressionSessionInvalidate` — this guarantees
     no further callbacks can fire.
   - **Then** release the session (CFRelease via `CfOwned::Drop`).
   - **Then** reclaim the box with `Box::from_raw(refcon)`. Order
     matters: reclaiming before Invalidate is UB if a callback is in
     flight.

### Synchronous vs asynchronous decode

`VTDecompressionSessionDecodeFrame(session, sample, decodeFlags, sourceRefCon, infoFlags)`:

- **decodeFlags = 0**: synchronous. The call blocks until the callback
  has been invoked. After `DecodeFrame` returns, the queue is populated.
- **decodeFlags = kVTDecodeFrame_EnableAsynchronousDecompression**:
  async. The call returns immediately; the callback fires later on VT's
  queue. Requires a separate `WaitForAsynchronousFrames` before destruction.

The sync mode is simpler and is what the safe wrapper uses; for
low-latency live decode the async mode gives better pipelining.

The `sourceFrameRefCon` argument is opaque to VT — it's passed through to
the callback so you can correlate the output back to the input that
produced it. Casting an `i64 PTS` to `*mut c_void` and reading it back as
`source_frame_ref_con as i64` is a common shortcut to avoid extracting
the PTS from the CMTime.

**Implementation:** [crates/oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs) — `DecompressionSession`,
`output_callback_trampoline`.

## CMBlockBuffer and CMSampleBuffer

To submit compressed bytes to VT you wrap them in two nested CoreMedia
types:

```text
  CMSampleBuffer            ← "1 frame of media, with timing + format"
  ├─ format description     ← CMVideoFormatDescription (the codec config)
  ├─ timing info            ← duration / PTS / DTS as CMTimes
  ├─ sample sizes           ← byte counts per sample (just one for video)
  └─ data buffer:
      CMBlockBuffer         ← "this many bytes, somewhere"
       └─ memory region     ← either CM-allocated or borrowed via custom block source
```

Building one from a Rust `&[u8]` of AVCC bytes:

1. `CMBlockBufferCreateWithMemoryBlock(memoryBlock=NULL, blockLength=N, …)`
   — CM allocates a fresh backing buffer.
2. `CMBlockBufferReplaceDataBytes(src=our_bytes, dst=block, …)` —
   memcpy our payload into CM's buffer.
3. `CMSampleBufferCreateReady(block, format, numSamples=1, …, timing,
   sampleSizes, …)` — wrap the block + format + timing.
4. Hand the CMSampleBuffer to `VTDecompressionSessionDecodeFrame`.

The "copy through CM" pattern avoids the lifetime gymnastics of borrowing
the Rust slice across the FFI boundary. The CM-allocated buffer is
released when its CMBlockBuffer wrapper drops, which happens automatically
when the surrounding CMSampleBuffer drops, which happens when our
`CfOwned` for the sample drops at end of scope.

**Implementation:** [crates/oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs) — `create_block_buffer`,
`create_sample_buffer`.

## Cross-references

|Concept|Code|
|---|---|
|H.264 NAL framing|[oximedia-vtb/src/nal.rs](../crates/oximedia-vtb/src/nal.rs)|
|RTP packet parser + sequence math|[oximedia-net/src/rtsp/rtp.rs](../crates/oximedia-net/src/rtsp/rtp.rs)|
|TCP-interleaved framing|[oximedia-net/src/rtsp/transport.rs](../crates/oximedia-net/src/rtsp/transport.rs)|
|SDP parser|[oximedia-net/src/rtsp/sdp.rs](../crates/oximedia-net/src/rtsp/sdp.rs)|
|HTTP Digest + inline MD5|[oximedia-net/src/rtsp/auth.rs](../crates/oximedia-net/src/rtsp/auth.rs)|
|CoreFoundation RAII|[oximedia-vtb/src/cf.rs](../crates/oximedia-vtb/src/cf.rs)|
|CMTime / CMSampleBuffer construction|[oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs)|
|VT decode callback bridge|[oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs)|
|NV12 plane extraction|[oximedia-vtb/src/session.rs](../crates/oximedia-vtb/src/session.rs)|
