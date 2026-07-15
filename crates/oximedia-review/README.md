# oximedia-review

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Collaborative review and approval workflow for OxiMedia. Provides comprehensive review and approval capabilities for video content including frame-accurate annotations, real-time collaboration, version comparison, and multi-stage approval workflows.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 | Tests: extensively tested — 2026-07-14

## Features

- **Frame-accurate Comments** - Annotations and comments tied to specific frames
- **Real-time Collaboration** - Multiple reviewers working simultaneously
- **Version Comparison** - Side-by-side A/B comparison with wipe and split tools
- **Multi-stage Approval** - Simple, parallel, sequential, and multi-stage workflows
- **Task Assignment** - Track and assign review tasks to team members
- **Drawing Tools** - Visual markup and annotation overlay
- **Notification System** - Email and webhook notifications
- **Export Capabilities** - Export reports as PDF, CSV, or EDL
- **Review Templates** - Reusable review checklist templates
- **Priority Management** - Prioritize review items
- **Timeline Notes** - Time-range annotations on media timeline
- **Feedback Rounds** - Track multiple rounds of review feedback
- **Change Tracking** - Version-based change diffing

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-review = "0.2.0"
```

```rust
use oximedia_review::{ReviewSession, SessionConfig, AnnotationType, WorkflowType};

// Create a review session
let config = SessionConfig::builder()
    .title("Final Cut Review")
    .content_id("video-123")
    .workflow_type(WorkflowType::MultiStage)
    .build();

let session = ReviewSession::create(config).await?;

// Add a frame-accurate comment
session.add_comment(
    1000, // frame number
    "Please adjust color grading",
    AnnotationType::Issue,
).await?;

// Invite reviewers
session.invite_user("reviewer@example.com").await?;
```

## API Overview

- `ReviewSession` — Core review session with comment, invite, and approval methods
- `SessionConfig` / `SessionConfigBuilder` — Session configuration builder
- `WorkflowType` — Simple, MultiStage, Parallel, Sequential
- `AnnotationType` — General, Issue, Suggestion, Question, Approval, Rejection
- `UserRole` — Owner, Approver, Reviewer, Observer
- `SessionId` / `CommentId` / `DrawingId` / `TaskId` / `VersionId` — Typed UUID identifiers
- `MediaComparator` / `CompareLayout` — Version comparison tools
- `TimelineNote` / `NoteType` / `TimeRange` — Timeline-based annotations
- Modules: `annotation`, `annotations`, `approval`, `approval_workflow`, `change`, `comment`, `compare`, `delivery`, `drawing`, `export`, `feedback_round`, `marker`, `notify`, `realtime`, `report`, `review_checklist`, `review_export`, `review_metrics`, `review_priority`, `review_session`, `review_status`, `review_tag`, `review_template`, `session`, `status`, `task`, `timeline_note`, `version`, `version_compare`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
