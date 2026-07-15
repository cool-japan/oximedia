# oximedia-monitor

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Comprehensive system monitoring and alerting for OxiMedia, providing professional-grade infrastructure monitoring with metrics collection, alerting, dashboards, REST API, and real-time streaming.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **System Metrics** — CPU, memory, disk, network, GPU, temperature monitoring
- **Application Metrics** — Encoding throughput, job statistics, worker status
- **Quality Metrics** — Bitrate, quality scores (PSNR, SSIM, VMAF)
- **Time Series Storage** — In-memory ring buffer with SQLite historical storage
- **Alerting** — Multiple channels (email, Slack, Discord, webhook, SMS, file)
- **Alert Rules** — Threshold-based and anomaly-detection alert rules
- **REST API** — Query metrics, manage alerts, health checks
- **WebSocket** — Real-time metric streaming
- **Health Checks** — Component health monitoring
- **Log Aggregation** — Structured logging with search and filtering
- **Dashboards** — Data provider for external visualization tools
- **Prometheus** — Compatible exposition format for metrics export
- **SLA/SLO Tracking** — Service level objective monitoring
- **Anomaly Detection** — Statistical anomaly detection for metrics
- **Capacity Planning** — Resource forecast and capacity planning
- **Incident Tracking** — Incident management and tracking
- **Event Bus** — Internal event routing for monitoring pipeline
- **Metric Export** — Export metrics to external systems
- **Uptime Tracking** — Service uptime calculation and reporting

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-monitor = "0.2.0"
```

```rust
use oximedia_monitor::{MonitorConfig, OximediaMonitor};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = MonitorConfig::default();
    let monitor = OximediaMonitor::new(config).await?;

    // Start monitoring
    monitor.start().await?;

    // Get system metrics
    if let Some(system_metrics) = monitor.system_metrics().await? {
        println!("CPU Usage: {:.2}%", system_metrics.cpu.total_usage);
    }

    Ok(())
}
```

## API Overview

**Core types:**
- `OximediaMonitor` — Main monitoring instance
- `MonitorConfig` — Configuration (storage path, API port, alert channels)

**Modules:**
- `alert`, `alert_rule` — Alert definitions and rules
- `alerting_pipeline` — Alert processing pipeline
- `anomaly` — Statistical anomaly detection
- `api` — REST API server
- `capacity_planner` — Resource capacity planning
- `config` — Configuration management
- `correlation` — Metric correlation analysis
- `counter_metrics` — Counter-based metrics
- `dashboard`, `dashboard_metric`, `dashboard_widget`, `panel_view` — Dashboard components
- `error` — Error types
- `event_bus` — Internal event routing
- `health`, `health_check` — Health check management
- `incident_tracker` — Incident management
- `integration` — External system integrations
- `log_aggregator`, `logs` — Log collection and search
- `metric_export`, `metric_pipeline`, `metric_store` — Metric storage and export
- `metrics` — Core metric types
- `reporting` — Report generation
- `resource_forecast` — Resource usage forecasting
- `retention` — Data retention policies
- `simple` — Simplified monitoring API
- `sla`, `slo_tracker` — SLA/SLO monitoring
- `storage` — Persistent storage backend
- `system_metrics` — System resource metrics
- `trace_span` — Distributed tracing spans
- `uptime_tracker` — Uptime tracking and calculation

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
