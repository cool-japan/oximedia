# oximedia-search

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Advanced media search and indexing engine for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.1 | Tests: extensively tested — 2026-08-12

## Overview

`oximedia-search` provides comprehensive search capabilities for media asset management, including full-text search, visual similarity, audio fingerprinting, faceted search, and advanced query processing.

## Features

- **Full-text Search** - Search metadata, transcripts, descriptions with fuzzy matching
- **Visual Search** - Find similar images and video frames using perceptual hashing
- **Audio Fingerprinting** - Identify and match audio content using patent-free algorithms
- **Faceted Search** - Filter by multiple criteria (codec, resolution, duration, etc.)
- **Boolean Queries** - Support for AND, OR, NOT operators
- **Range Queries** - Search by date ranges, duration ranges, numeric ranges
- **Reverse Search** - Find clips from sample frames or audio snippets
- **Color Search** - Search by dominant colors or color palettes
- **Face Search** - Find people in videos using face recognition
- **OCR Search** - Search text visible in video frames
- **Relevance Ranking** - BM25 and TF-IDF scoring
- **Semantic Search** - Semantic/embedding-based similarity search
- **Spell Suggestions** - Query spell correction and suggestions
- **Search Federation** - Federated search across multiple indices
- **Search Clustering** - Distributed search cluster support
- **Search Analytics** - Query analytics and reporting
- **Search Rewriting** - Query expansion and rewriting
- **Search Pipeline** - Composable search processing pipeline
- **Temporal Search** - Time-based media search queries
- **Inverted Index** - Efficient inverted index implementation
- **Search Caching** - Result caching for performance

## Usage

```rust
use oximedia_search::{SearchEngine, SearchQuery, SearchFilters, SortOptions};
use std::path::Path;

// Create a search engine
let engine = SearchEngine::new(Path::new("/path/to/index"))?;

// Create a search query
let query = SearchQuery {
    text: Some("breaking news".to_string()),
    visual: None,
    audio: None,
    filters: SearchFilters {
        mime_types: vec!["video/mp4".to_string()],
        codecs: vec!["h264".to_string()],
        duration_range: Some((60000, 300000)), // 1-5 minutes
        ..Default::default()
    },
    limit: 20,
    offset: 0,
    sort: SortOptions::default(),
};

// Execute search
let results = engine.search(&query)?;
println!("Found {} results", results.total);
```

## API Overview

- `SearchEngine` — Main search coordinator: text, visual, audio, face, OCR, color indices
- `SearchQuery` — Unified query: text, visual, audio, filters, sort, pagination
- `SearchFilters` — MIME types, codecs, resolutions, duration/date/size ranges, colors, keywords
- `SearchResults` / `SearchResultItem` — Paginated results with facets and query time
- `SortOptions` / `SortField` / `SortOrder` — Flexible result ordering
- `SearchError` / `SearchResult` — Error and result types
- Modules: `audio`, `bool_query`, `cache`, `color`, `error`, `face`, `facet`, `facets`, `index`, `index_builder`, `index_stats`, `inv_index`, `media_index`, `ocr`, `query`, `query_parser`, `range`, `rank`, `ranking`, `relevance_score`, `reverse`, `search_analytics`, `search_cluster`, `search_federation`, `search_filter`, `search_pipeline`, `search_ranking`, `search_result`, `search_rewrite`, `search_suggest`, `semantic`, `spell_suggest`, `temporal`, `text`, `visual`

## Integration with OxiMedia

- **oximedia-mam**: Extends MAM search capabilities
- **oximedia-cv**: Uses visual features and fingerprinting
- **oximedia-scene**: Leverages scene classification and object detection

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
