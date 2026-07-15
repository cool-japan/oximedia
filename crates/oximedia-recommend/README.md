# oximedia-recommend

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Content recommendation and discovery engine for OxiMedia. Provides comprehensive recommendation capabilities including content-based filtering, collaborative filtering, hybrid approaches, and advanced personalization.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Content-based Filtering** - Recommend similar content based on features
- **Collaborative Filtering** - User behavior-based recommendations via matrix factorization (SVD)
- **Hybrid Approach** - Combine multiple recommendation methods
- **User Profiles** - Build and manage user preference profiles
- **View History** - Track and analyze viewing patterns
- **Rating System** - Handle explicit ratings and implicit feedback
- **Trending Detection** - Identify trending content in real-time
- **Personalization** - Context-aware personalized recommendations
- **Diversity Enforcement** - Ensure recommendation diversity and avoid filter bubbles
- **Freshness Balancing** - Balance popular and new content
- **A/B Testing** - Experimentation framework for recommendation strategies
- **Bandit Algorithms** - Multi-armed bandit for exploration-exploitation
- **Cold Start** - Strategies for new users and new content
- **Sequence Models** - Sequential recommendation based on viewing history
- **Exploration Policy** - Configurable exploration strategies
- **Decay Model** - Time-based relevance decay
- **Score Cache** - Cached recommendation scores
- **Feature Store** - User and item feature management
- **Feedback Signal** - Implicit and explicit feedback processing
- **Impression Tracker** - Impression and click tracking
- **Item Similarity** - Item-to-item similarity computation
- **Popularity Bias** - Popularity de-biasing
- **Ranking** - Multi-objective ranking
- **Session** - Session-based recommendations

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-recommend = "0.2.0"
```

```rust
use oximedia_recommend::{RecommendationEngine, RecommendationRequest, RecommendationStrategy};
use uuid::Uuid;

let mut engine = RecommendationEngine::new();

// Get recommendations for a user
let request = RecommendationRequest {
    user_id: Uuid::new_v4(),
    limit: 10,
    strategy: RecommendationStrategy::Hybrid,
    ..Default::default()
};

let results = engine.recommend(&request)?;

// Record user interaction
engine.record_view(user_id, content_id, watch_time_ms, completed)?;
engine.record_rating(user_id, content_id, 4.5)?;
```

## API Overview

**Core types:**
- `RecommendationEngine` — Main engine coordinating all recommendation strategies
- `RecommendationRequest` — Request with user ID, limit, strategy, and context
- `RecommendationStrategy` — ContentBased, Collaborative, Hybrid, Personalized, Trending
- `Recommendation` / `RecommendationReason` — Result item with score and explanation
- `DiversitySettings` — Category diversity and serendipity configuration
- `RecommendationContext` — Device, location, time-of-day context

**Modules:**
- `ab_test` — A/B testing framework
- `bandits` — Multi-armed bandit algorithms
- `calibration` — Recommendation calibration
- `cold_start` — Cold start strategies
- `collab_filter`, `collaborative` — Collaborative filtering (filter, predict, SVD)
- `content`, `content_based` — Content-based filtering
- `context_signal` — Context signal processing
- `decay_model` — Time-based decay
- `diversity` — Diversity enforcement
- `error` — Error types
- `explain` — Recommendation explanation generation
- `exploration_policy` — Exploration strategies
- `feature_store` — Feature management
- `feedback_signal` — Feedback processing
- `freshness` — Content freshness scoring
- `history` — View/listen history tracking
- `hybrid` — Hybrid recommendation strategies
- `impression_tracker` — Impression and click tracking
- `item_similarity` — Item similarity computation
- `personalize` — Personalization engine
- `popularity_bias` — Popularity de-biasing
- `profile` — User preference profiles
- `rank`, `ranking` — Multi-objective ranking
- `rating` — Explicit rating handling
- `recommendation_score` — Score computation
- `score_cache` — Score caching
- `sequence_model` — Sequential recommendation
- `session` — Session-based recommendations
- `trending` — Trending content detection
- `user_profile` — User profile management

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
