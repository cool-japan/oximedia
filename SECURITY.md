# Security Policy

OxiMedia parses and processes **untrusted, attacker-controlled media** —
container files (MP4, Matroska/WebM, MPEG-TS, OGG, WAV, ...), bitstreams
(AV1, VP9, VP8, Theora, Opus, Vorbis, FLAC, ...), and network protocol
traffic (HLS, DASH, RTMP, SRT, WebRTC, SMPTE 2110). This is exactly the
category of code that has historically been the largest source of memory
corruption vulnerabilities in C/C++ media frameworks (FFmpeg, libav,
GStreamer, and OpenCV all have long CVE histories in their parsers and
decoders). Security reports against this codebase are taken seriously and
are very welcome.

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: (latest release) |
| < 0.1   | :x: (pre-release / superseded)      |

OxiMedia is pre-1.0 and evolves quickly across `0.1.x` releases. Security
fixes are made against the latest released `0.1.x` version on the `master`
branch (releases are tagged `Availability of X.Y.Z`); we do not maintain
long-term-support branches for older patch releases at this stage. If you
are running an older `0.1.x` release, please upgrade to the latest before
filing a report, or state clearly in your report if you believe the issue
is version-specific.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities privately via **GitHub Security Advisories**:

**https://github.com/cool-japan/oximedia/security/advisories/new**

This project does not currently operate a dedicated security-contact email
alias; GitHub Security Advisories is the authoritative and only channel for
private disclosure, and it is monitored by the maintainers (COOLJAPAN OU /
Team KitaSan).

When filing a report, please include:

- A description of the vulnerability and its impact (e.g. out-of-bounds
  read/write, panic/DoS, integer overflow leading to undersized allocation,
  logic bug affecting a security-relevant decision such as DRM key handling
  or path handling).
- The affected crate(s) and, if known, file/function.
- A minimal reproduction: for parser/decoder issues, a crafted input file
  (or a short program that constructs the problematic byte sequence) plus
  the exact API call that triggers it.
- The OxiMedia version or commit hash, and your Rust toolchain version.
- Whether you believe the issue is exploitable beyond a panic/crash (e.g.
  memory disclosure or corruption via `unsafe` code), and your reasoning.

### Responsible Disclosure Expectations

- This is a small open-source project maintained on a best-effort basis —
  please give us a reasonable window to investigate and ship a fix before
  any public disclosure. We aim to acknowledge new reports within a few
  days and will work with you on a disclosure timeline; we don't currently
  commit to a fixed SLA (e.g. a strict N-day patch guarantee), but will
  communicate progress and timeline expectations once a report is
  triaged.
- We will credit reporters (by name/handle, unless you ask to stay
  anonymous) in the fix's changelog entry and/or the GitHub Security
  Advisory, once a fix is public.
- If a reported issue affects the wider COOLJAPAN Pure Rust ecosystem (a
  dependency such as `oxiarc-*`, `oxisql-*`, `oxicode`, `oxifft`, `oxiblas`,
  or `oxionnx`), we will help route the report to that project as well.

## Scope

**In scope:**

- Memory-safety issues in any crate: out-of-bounds access, use-after-free,
  data races, or undefined behavior — including inside the `unsafe` blocks
  that exist for SIMD intrinsics, `mmap`-backed I/O (e.g. `oximedia-repair`,
  `oximedia-archive`), and lock-free data structures (crates that opt out of
  the workspace-wide `unsafe_code = "deny"` lint: `oximedia-accel`,
  `oximedia-gpu`, `oximedia-simd`, `oximedia-routing`, `oximedia-switcher`).
- Parser/decoder robustness against malformed or adversarial input: crashes,
  panics, infinite loops, unbounded memory/CPU consumption (denial of
  service), or incorrect output with security relevance, when decoding
  container files or bitstreams. The `fuzz/` directory already targets many
  of the highest-risk parsers (MP4, Matroska/EBML, OGG, WAV, FLAC, Vorbis,
  Opus, AV1, VP8, VP9, Theora, HLS, DASH, RTMP) — fuzzing findings that
  aren't yet covered by an existing target are especially useful, as are new
  `cargo-fuzz` targets for parsers that don't have one yet.
- Logic bugs with a genuine security consequence: e.g. DRM/key handling
  (`oximedia-drm`), authentication/authorization bypass in the media server
  (`oximedia-server`), path traversal or unsafe deserialization when reading
  project/archive metadata.
- Supply-chain issues: a malicious or compromised transitive dependency.
  Known, already-triaged advisories for transitive dependencies are tracked
  with documented rationale in `audit.toml` (cargo-deny) and
  `.cargo/audit.toml` (cargo-audit) — please check there first in case an
  issue is already known and intentionally ignored (with a stated reason)
  because the vulnerable code path is unreachable from OxiMedia's usage.

**Out of scope / lower priority:**

- Vulnerabilities that require the attacker to already control the local
  machine running OxiMedia, or that require an already-compromised build
  toolchain.
- Denial-of-service reports that only apply when OxiMedia is deliberately
  fed a pathological input **and the calling application is expected to
  impose its own resource limits** (as is normal practice for any media
  pipeline accepting untrusted input) — but please still report it, since we
  do want to document and, where reasonable, harden against these cases.
- Issues purely in example code under `examples/` or in benchmark harnesses
  under `benches/`, unless they demonstrate a real vulnerability in a
  library crate.

## Project Security Posture

- **Memory safety by construction.** The workspace denies `unsafe_code` at
  the lint level by default (`[workspace.lints.rust]` in the root
  `Cargo.toml`); only a small, explicitly named set of crates opts back into
  `unsafe` for SIMD intrinsics, `mmap`, or lock-free structures (see
  "Scope" above). This removes an entire class of vulnerabilities (buffer
  overflows, use-after-free) from the ~100+ crates that never need `unsafe`
  at all — but it does not make the remaining `unsafe` blocks automatically
  correct, and those are the crates where memory-safety reports are most
  valuable.
- **No `unwrap()`/`expect()` in production code.** Error paths are expected
  to return `Result` rather than panic, which matters for availability
  (a malformed input should produce an `Err`, not take down the process).
  If you find a reachable `unwrap()`/`expect()`/array-index panic on
  attacker-controlled input, that is a valid — and easy — report.
- **Fuzzing.** `fuzz/` contains `cargo-fuzz` targets for the highest-risk
  container/codec parsers (see the list above). It is intentionally
  excluded from the main workspace build (`exclude = ["fuzz"]` in the root
  `Cargo.toml`) and is run separately.
- **Dependency auditing.** `audit.toml` and `.cargo/audit.toml` track every
  currently-ignored RUSTSEC advisory for transitive dependencies, each with
  a written rationale for why the vulnerable code path is unreachable from
  OxiMedia and a note on what upstream fix would resolve it. New advisories
  that are *not* already listed there should be reported/flagged rather than
  assumed to be known.
- **Honesty about maturity.** Not every subsystem is production-hardened.
  `docs/codec_status.md` documents, per video/audio codec, whether decode is
  **Verified** (matches a reference decoder), **Functional** (self-consistent,
  no third-party conformance proof yet), **Bitstream-parsing** only (headers
  parsed, pixel/sample output stubbed or partial), or **Experimental** (API
  sketch, not intended to actually decode). The same honesty standard is
  meant to apply project-wide, including subsystems outside codecs (for
  example, some network/streaming protocol paths involving encryption or
  authentication handshakes) — if you find code whose real maturity doesn't
  match what its documentation or public API surface implies, please treat
  that mismatch itself as worth reporting (it is a security-relevant
  documentation bug even before it's a code bug), and check
  `docs/codec_status.md` first for the vocabulary this project uses to talk
  about it.
- **Patent-free codecs by default.** Not a memory-safety property, but part
  of the project's threat model: OxiMedia defaults to royalty-free codecs
  (AV1, VP9, Opus, FLAC, etc.) specifically to avoid entangling downstream
  users in patent risk.

## Response Time Expectations

This is a best-effort, community-maintained project. There is no funded
security team and no contractual SLA. As a rough guide:

- **Acknowledgement**: within a few days of a report via GitHub Security
  Advisories.
- **Triage and severity assessment**: as soon as practical after
  acknowledgement, typically within one to two weeks.
- **Fix and release**: timeline depends on severity and complexity; we will
  communicate an estimate once triage is complete, and will prioritize
  memory-safety issues reachable from untrusted input (parsers/decoders)
  above all else.

Thank you for helping keep OxiMedia and its users safe.
