# oximedia-dedup

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Media deduplication and duplicate detection for OxiMedia, providing cryptographic, visual, audio, and metadata-based duplicate finding with SQLite-backed indexing.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Cryptographic Hashing** — BLAKE3-based exact duplicate detection
- **Visual Similarity** — Perceptual hashing, SSIM, histogram matching, and feature matching
- **Audio Fingerprinting** — Audio fingerprint comparison and waveform similarity
- **Metadata Matching** — Fuzzy metadata comparison for near-duplicates
- **Rolling Hash** — Segment-level deduplication for partial matches
- **LSH Index** — Locality-sensitive hashing for fast approximate nearest neighbor search
- **Bloom Filter** — Probabilistic fast duplicate screening
- **Cluster Analysis** — Group similar media into clusters
- **Storage Optimization** — SQLite-based indexing for large libraries
- **Comprehensive Reporting** — Duplicate reports with similarity scoring
- **Content ID** — Content-based identity tracking
- **Content Signatures** — Robust perceptual signatures
- **Dedup Policy** — Configurable dedup policies
- **Fuzzy Matching** — Fuzzy metadata and filename matching
- **Merge Strategy** — `MergeExecutor::apply()` / `dry_run()` with `AppliedAction { Symlinked, Hardlinked, Deleted, Kept, Skipped }` and `MergeReport`; Unix symlink + hardlink; Windows symlink_file with fallback
- **Segment Dedup** — Segment-level partial matching
- **Similarity Index** — Fast similarity index
- **Video Dedup** — Video-specific deduplication

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-dedup = "0.2.0"
```

```rust
use oximedia_dedup::{DuplicateDetector, DetectionStrategy, DedupConfig};

async fn example() -> Result<(), Box<dyn std::error::Error>> {
    let config = DedupConfig::default();
    let mut detector = DuplicateDetector::new(config).await?;

    detector.add_file("/path/to/video1.mp4").await?;
    detector.add_file("/path/to/video2.mp4").await?;

    let duplicates = detector.find_duplicates(DetectionStrategy::All).await?;
    Ok(())
}
```

## API Overview

**Core types:**
- `DuplicateDetector` — Main deduplication engine
- `DedupConfig` — Configuration (thresholds, paths, parallel mode)
- `DetectionStrategy` — ExactHash / PerceptualHash / Ssim / Histogram / FeatureMatch / AudioFingerprint / Metadata / All / VisualAll / Fast
- `DuplicateGroup`, `DuplicateReport`, `SimilarityScore` — Results
- `DedupDatabase` — SQLite index backend
- `DedupStats` — Index statistics

**Modules:**
- `hash`, `frame_hash`, `rolling_hash` — Hashing strategies
- `visual`, `perceptual_hash`, `phash` — Visual similarity
- `audio` — Audio fingerprint comparison
- `metadata` — Metadata-based matching
- `database`, `hash_store`, `dedup_index` — Storage backends
- `lsh_index` — Locality-sensitive hashing index
- `bloom_filter` — Probabilistic screening
- `cluster` — Similarity clustering
- `near_duplicate` — Near-duplicate detection
- `report`, `dedup_report`, `dedup_report_ext` — Reporting
- `content_id`, `content_signature` — Content identity
- `dedup_cache`, `dedup_policy` — Caching and policy
- `dedup_stats` — Statistics
- `fuzzy_match` — Fuzzy matching
- `merge_strategy` — `MergeExecutor`, `AppliedAction`, `MergeReport` — real FS duplicate resolution
- `segment_dedup` — Segment dedup
- `similarity_index` — Similarity index
- `video_dedup` — Video-specific dedup

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
