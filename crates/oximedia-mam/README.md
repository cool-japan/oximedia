# oximedia-mam

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 983](https://img.shields.io/badge/tests-983-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

Media Asset Management (MAM) system for OxiMedia, providing PostgreSQL-backed asset storage, full-text search, workflow engines, and comprehensive media library management.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Asset Management** — Full asset lifecycle with status tracking, tagging, and versioning
- **PostgreSQL Backend** — Scalable relational storage via SQLx with connection pooling
- **Full-text Search** — Tantivy-powered full-text and faceted search
- **REST and GraphQL APIs** — Actix-web REST API and async-graphql interface
- **Workflow Engine** — Approval and review workflow with event-driven triggers
- **Collection Management** — Hierarchical collection and folder organization
- **Asset Ingest** — Metadata extraction, pipeline processing, and bulk ingest
- **Proxy and Thumbnail** — Automatic proxy and thumbnail generation
- **User Permissions** — RBAC permission management
- **Cloud Storage** — S3, Azure, GCS integration
- **Webhook and Events** — Webhook notifications and event-driven automation
- **Tag Management** — Hierarchical tag taxonomy with inverted index
- **Audit Logging** — Full operation audit trail
- **Folder Structure** — Hierarchical folder organization with smart collections
- **Asset Relations** — Relationship graph between assets
- **Media Linking** — Link media files to assets
- **Retention Policies** — Automated asset lifecycle and retention management
- **Rights Management** — Rights coverage summary and rights tracking
- **Usage Analytics** — Access pattern and usage analytics
- **Delivery Log** — Asset delivery history tracking
- **Transfer Manager** — Managed file transfers with retry
- **Metadata Templates** — Structured metadata schema templates
- **Integration** — Third-party system integration

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-mam = "0.2.0"
```

```rust
use oximedia_mam::{MamSystem, MamConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = MamConfig::default();
    let mam = MamSystem::new(config).await?;

    let health = mam.health().await?;
    println!("Assets: {}", health.asset_count);
    println!("DB healthy: {}", health.database);

    mam.shutdown().await?;
    Ok(())
}
```

## API Overview

**Core types:**
- `MamSystem` — Main system coordinator
- `MamConfig` — System configuration (database_url, index_path, storage_path, proxy_path, jwt_secret)
- `MamError`, `Result` — Error types
- `HealthStatus` — System health indicators
- `SystemStatistics` — Aggregate system statistics

**Asset modules:**
- `asset` — `AssetManager` and asset CRUD
- `asset_collection` — Asset collection membership
- `asset_lifecycle` — Asset lifecycle state machine
- `asset_relations` — Asset relationship graph
- `asset_search` — Structured asset search
- `asset_status` — Asset status tracking
- `asset_tag`, `asset_tag_index` — Asset tagging with inverted index
- `asset_tagging` — Automatic and manual tagging

**Ingest modules:**
- `ingest` — `IngestSystem`
- `ingest_pipeline` — Multi-stage ingest pipeline
- `ingest_workflow` — Ingest workflow integration
- `batch_ingest` — Bulk ingest processing
- `bulk_operation` — Bulk asset operations

**Search modules:**
- `search` — `SearchEngine` (Tantivy full-text)
- `search_index` — Search index management
- `catalog_search` — Catalog search with field-level filters

**Collection and organization:**
- `collection` — `CollectionManager`
- `collection_manager` — Collection operations
- `folder_hierarchy` — Hierarchical folder tree
- `folders` — `FolderManager`

**Workflow modules:**
- `workflow` — `WorkflowEngine`
- `workflow_integration` — External workflow integration
- `workflow_trigger` — Event-driven workflow triggers

**Proxy and delivery:**
- `proxy` — `ProxyManager`
- `delivery_log` — Delivery history

**Rights and compliance:**
- `rights_summary` — Rights coverage builder
- `retention_policy` — Retention policy engine
- `media_linking` — Asset-to-file linking

**Storage and transfer:**
- `storage` — `StorageManager` (local/S3/Azure/GCS)
- `transfer_manager` — Managed file transfers

**Metadata and catalog:**
- `media_catalog` — Media catalog operations
- `media_format_info` — Media format registry
- `media_project` — Media project lifecycle
- `metadata_template` — Metadata template library
- `transcoding_profile` — Transcoding profile management

**User and access:**
- `permissions` — `PermissionManager` / RBAC
- `audit` — `AuditLogger`

**Tags and webhooks:**
- `tags` — `TagManager` with hierarchical taxonomy
- `webhook` — `WebhookManager`

**Versioning and analytics:**
- `version_control` — Asset version control
- `versioning` — Version metadata
- `usage_analytics` — Access pattern analytics
- `export_package` — Export package builder

**API and integration:**
- `api` — REST and GraphQL API handlers
- `integration` — `IntegrationManager`
- `database` — `Database` with connection pool and migrations

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
