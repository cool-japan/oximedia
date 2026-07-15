# oximedia-rights

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Content rights and licensing management for OxiMedia. Provides comprehensive rights management including ownership tracking, license management, territory restrictions, royalty calculation, DRM metadata, and compliance reporting.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 | Tests: extensively tested — 2026-07-14

## Features

- **Rights Tracking** - Ownership and rights holder management
- **License Management** - Royalty-free, rights-managed, and custom license types
- **Expiration Tracking** - Monitor and alert on rights expiration
- **Territory Restrictions** - Geographic usage restrictions and distribution windows
- **Usage Tracking** - Track and report content usage
- **Clearance Tracking** - Music, footage, and talent clearance workflows
- **Royalty Calculation** - Automatic royalty calculation and payment tracking
- **Watermarking Integration** - Link rights to watermark systems
- **DRM Metadata** - Digital rights management metadata management
- **Audit Trail** - Comprehensive audit logging for compliance
- **Embargo Policies** - Time-based content embargoes with window management
- **Syndication Rights** - Manage syndication and distribution agreements
- **Rights Conflict Detection** - Identify and resolve conflicting rights claims
- **License Templates** - Reusable license template management
- **Rights Negotiation** - Workflow for rights negotiation and approval
- **Rights Bundles** - Group related rights for batch management

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-rights = "0.2.0"
```

```rust
use oximedia_rights::RightsManager;

// Create a rights manager with SQLite database
let manager = RightsManager::new("rights.db").await?;

// Access the rights database
let db = manager.database();
```

## API Overview

- `RightsManager` — Main rights management system backed by SQLite (Pure-Rust engine via OxiSQL — no C/C++ compiled)
- `RightsError` / `Result` — Error and result types
- Modules: `audit`, `clearance`, `clearance_workflow`, `compliance`, `contract`, `database`, `distribution_rights`, `distribution_window`, `drm`, `embargo`, `embargo_policy`, `embargo_window`, `expiration`, `license`, `license_template`, `licensing_model`, `licensing_terms`, `registry`, `rights`, `rights_audit_trail`, `rights_bundle`, `rights_check`, `rights_conflict`, `rights_database`, `rights_holder`, `rights_negotiation`, `rights_timeline`, `royalty`, `royalty_calc`, `royalty_schedule`, `sync_rights`, `syndication`, `territory`, `usage`, `usage_report`, `usage_rights`, `watermark`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
