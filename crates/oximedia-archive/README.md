# oximedia-archive

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Media archive verification and long-term preservation system for OxiMedia. Provides checksumming, fixity checking, OAIS compliance, PREMIS event logging, quarantine management, and comprehensive verification reporting.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Multi-algorithm Checksums** — BLAKE3, SHA-256, MD5, and CRC32 verification
- **Sidecar Files** — Generate checksum sidecar files alongside media
- **Fixity Checking** — Scheduled periodic integrity verification
- **Container Validation** — Validate media file structure and metadata
- **PREMIS Logging** — Digital preservation event logging
- **BagIt Support** — BagIt package creation and verification
- **Quarantine Management** — Isolate corrupted or suspect files
- **SQLite Database** — Persistent verification history via sqlx
- **Parallel Verification** — Multi-threaded verification for large archives via rayon
- **Catalog Management** — Archive catalog with search and indexing
- **Migration Support** — Format migration planning and execution
- **Retention Scheduling** — Configurable retention policies
- **Tape Support** — LTO tape archive management
- **Deduplication** — Content-based deduplication

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-archive = "0.2.0"
```

```rust
use oximedia_archive::{ArchiveVerifier, VerificationConfig};
use std::path::Path;

let config = VerificationConfig {
    enable_blake3: true,
    enable_sha256: true,
    generate_sidecars: true,
    ..Default::default()
};

let mut verifier = ArchiveVerifier::with_config(config);
verifier.initialize().await?;

// Verify a file
let result = verifier.verify_file(Path::new("media.mkv")).await?;
println!("Status: {:?}", result.status);
println!("BLAKE3: {:?}", result.checksums.blake3);

// Run scheduled fixity checks
verifier.run_fixity_checks().await?;
```

## API Overview (27 source files, 550 public items)

**Core types:**
- `ArchiveVerifier` — Main verifier with initialize, verify, and fixity check methods
- `VerificationConfig` — Configuration for checksum algorithms and features
- `VerificationResult` / `VerificationStatus` — Detailed verification outcome
- `ChecksumSet` — Set of checksums (BLAKE3, SHA-256, MD5, CRC32)

**Modules:**
- `checksum` — Multi-algorithm checksum computation (BLAKE3, SHA-256, MD5, CRC32)
- `fixity` — Scheduled fixity checking and integrity verification
- `validate` — Media container structure validation
- `catalog` — Archive catalog with search and indexing
- `preservation` — OAIS and PREMIS event logging
- `report` — Verification report generation
- `tape` — LTO tape archive management
- `migration` — Format migration planning and execution

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
