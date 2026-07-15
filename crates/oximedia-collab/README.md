# oximedia-collab

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Real-time CRDT-based multi-user collaboration system for OxiMedia, supporting concurrent video editing with sub-second synchronization.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-collab` provides a comprehensive CRDT-based synchronization system supporting 10+ concurrent editors with sub-second latency. Built on Yjs (via `yrs`) for conflict-free merging, with WebSocket-based communication and offline-first architecture.

## Features

### CRDT Document Synchronization
- Yjs-based document synchronization via `yrs` (`CrdtDocument` backed by `yrs::Doc`)
- Classic CRDT primitives: GCounter, PNCounter, LWWRegister, MVRegister, GSet, TwoPhaseSet
- Operational transformation: Insert/Delete/Update/Move/Composite ops with FIFO tiebreak
- Delete/Delete sentinel (`usize::MAX`) and git-rebase-style `rebase()` for sequential transform
- `OpDag` with Kahn topological ordering and memoized LCA (`AncestorCache`) for causal ordering
- `DeltaChangeset`: `delta_from(base, current)` encodes only suffix ops beyond the base version
- Three-way merge (`ProjectMerger`) with five conflict strategies and heuristic score resolution
- Snapshot repository with parent-chain, BFS common-ancestor, and branch/fast-forward detection
- Version vector tracking for causality; Lamport timestamps and vector clocks

### Real-time Synchronization
- WebSocket-based communication protocol (tokio-tungstenite)
- Compact binary frame format: 4-byte header `[type u16 LE][len u16 LE]` + payload
- `BatchedFramer` auto-flushes buffered frames above a configurable byte threshold
- Delta encoding and adaptive bandwidth throttling (`bandwidth_throttle`)
- Offline support with change queue management (up to 10 000 entries)
- Reconnection strategies with exponential backoff
- Heartbeat/keep-alive mechanism and connection statistics

### Session Management
- Multi-user session coordination (up to 10 users per session)
- User presence tracking and active indicators
- Cursor and selection synchronization
- Session locking (who is editing what)
- Permission enforcement: Owner, Editor, Viewer roles
- Session metadata management and garbage collection

### Optimistic Locking
- Resource locking: clips, tracks, timeline, project; region-based and track-based scopes
- Read/Write lock types with RAII-style lock guards
- Lock stealing with permission checks (Owner role required)
- Timeout-based automatic release (configurable, default 5 minutes)
- Deadlock detection via `WaiterGraph` DFS cycle detection (`detect_cycle_if_added`)
- Returns full cycle path on deadlock; waiter edges removed on lock release

### Shared History
- Per-user undo/redo stack
- Cross-user history and history branching
- Change attribution and history compaction
- History visualization (ASCII timeline, DOT graphs)
- Import/export functionality

### Awareness Protocol
- Yjs awareness implementation
- User state broadcasting
- Cursor position and selection range synchronization
- Viewport state sharing and user color assignment
- Ephemeral state management with heartbeat

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-collab = "0.2.0"
```

```rust
use oximedia_collab::{CollaborationServer, CollabConfig, User, UserRole};

#[tokio::main]
async fn main() {
    let config = CollabConfig::default();
    let server = CollaborationServer::new(config);

    // Create session
    let owner = User::new("Alice".to_string(), UserRole::Owner);
    let project_id = uuid::Uuid::new_v4();
    let session_id = server.create_session(project_id, owner).await?;

    // Join session
    let editor = User::new("Bob".to_string(), UserRole::Editor);
    server.join_session(session_id, editor).await?;

    // Start background tasks (GC, heartbeat)
    server.start_background_tasks().await;

    // ... use the session ...

    server.shutdown().await?;
}
```

## Configuration

```rust
CollabConfig {
    max_users_per_session: 10,
    lock_timeout_secs: 300,           // 5 minutes
    enable_compression: true,
    compression_threshold: 1024,       // 1KB
    history_limit: 1000,
    gc_interval_secs: 600,            // 10 minutes
    enable_offline: true,
    max_offline_queue: 10000,
}
```

## Architecture

| Module | Purpose |
|--------|---------|
| `crdt` | CRDT operations, merging, conflict resolution |
| `crdt_primitives` | GCounter, PNCounter, LWWRegister, MVRegister, GSet, TwoPhaseSet |
| `operation_log` | OT transform/rebase, OpDag Kahn ordering, AncestorCache LCA |
| `changeset` | DeltaChangeset suffix encoding and apply |
| `binary_framer` | Compact binary frame format; BatchedFramer |
| `edit_lock` | Region/track/hierarchical locking with WaiterGraph deadlock detection |
| `three_way_merge` | ProjectMerger, ConflictResolution (5 strategies), heuristic scoring |
| `snapshot_manager` | Git-inspired snapshot repository with BFS common ancestor |
| `sync` | Network synchronization, WebSocket, reconnection |
| `history` | Undo/redo, history management |
| `session` | Session coordination, user management |
| `awareness` | Presence tracking, cursor synchronization |
| `user_presence_map` | Spatial cursor/viewport tracking |
| `lib` | Public API, CollaborationServer |

## Conflict Resolution Strategies

1. **Last-Write-Wins** — Timestamp-based resolution
2. **First-Write-Wins** — Original operation takes precedence
3. **User-ID-Wins** — Deterministic tiebreaker based on user ID
4. **Manual** — Requires explicit conflict resolution

## Performance

- **Latency**: Sub-second synchronization (typically <100ms)
- **Concurrency**: 10+ concurrent editors
- **Bandwidth**: Optimized with delta encoding and lz4/gzip compression
- **Memory**: Efficient with garbage collection and history limits

## Safety

- No unsafe code
- All shared state protected by `Arc<RwLock>`
- All errors handled via `Result` types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
