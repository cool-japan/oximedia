//! `oximedia.analytics` geo/device breakdown bindings — real delegation to
//! [`oximedia_analytics::geo_device`].
//!
//! Aggregates session records across geographic region and device-type
//! dimensions, and supports period-over-period comparison for a single
//! region or device slice.
//!
//! Regions and devices are passed as strings using the same canonical slugs
//! as the Rust `label()` methods (e.g. `"north_america"`, `"smart_tv"`).
//! Unknown slugs are rejected with a `ValueError` rather than silently
//! mapping to `Unknown`.

use oximedia_analytics::geo_device as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

fn parse_region(region: &str) -> PyResult<core::Region> {
    Ok(match region {
        "north_america" => core::Region::NorthAmerica,
        "latin_america" => core::Region::LatinAmerica,
        "europe" => core::Region::Europe,
        "eastern_europe" => core::Region::EasternEurope,
        "mea" => core::Region::Mea,
        "asia_pacific" => core::Region::AsiaPacific,
        "east_asia" => core::Region::EastAsia,
        "unknown" => core::Region::Unknown,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown region {other:?}; expected one of north_america, latin_america, \
                 europe, eastern_europe, mea, asia_pacific, east_asia, unknown"
            )))
        }
    })
}

fn parse_device(device: &str) -> PyResult<core::DeviceType> {
    Ok(match device {
        "desktop" => core::DeviceType::Desktop,
        "mobile" => core::DeviceType::Mobile,
        "tablet" => core::DeviceType::Tablet,
        "smart_tv" => core::DeviceType::SmartTv,
        "console" => core::DeviceType::Console,
        "unknown" => core::DeviceType::Unknown,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown device type {other:?}; expected one of desktop, mobile, tablet, \
                 smart_tv, console, unknown"
            )))
        }
    })
}

// ---------------------------------------------------------------------------
// SliceMetrics / PeriodDelta / SliceComparison
// ---------------------------------------------------------------------------

/// Aggregated metrics for a single dimension slice (a region or a device).
#[pyclass(name = "SliceMetrics")]
#[derive(Clone)]
pub struct PySliceMetrics {
    #[pyo3(get)]
    pub sessions: u64,
    #[pyo3(get)]
    pub unique_viewers: u64,
    #[pyo3(get)]
    pub total_watch_seconds: f64,
    #[pyo3(get)]
    pub avg_watch_seconds: f64,
}

impl From<&core::SliceMetrics> for PySliceMetrics {
    fn from(m: &core::SliceMetrics) -> Self {
        Self {
            sessions: m.sessions,
            unique_viewers: m.unique_viewers,
            total_watch_seconds: m.total_watch_seconds,
            avg_watch_seconds: m.avg_watch_seconds,
        }
    }
}

#[pymethods]
impl PySliceMetrics {
    fn __repr__(&self) -> String {
        format!(
            "SliceMetrics(sessions={}, unique_viewers={}, avg_watch_seconds={:.1})",
            self.sessions, self.unique_viewers, self.avg_watch_seconds
        )
    }
}

/// Period-over-period change for a single metric.
#[pyclass(name = "PeriodDelta")]
#[derive(Clone)]
pub struct PyPeriodDelta {
    #[pyo3(get)]
    pub baseline: f64,
    #[pyo3(get)]
    pub comparison: f64,
    #[pyo3(get)]
    pub absolute_change: f64,
    /// `NaN` when `baseline` is zero.
    #[pyo3(get)]
    pub relative_change_pct: f64,
}

impl From<&core::PeriodDelta> for PyPeriodDelta {
    fn from(d: &core::PeriodDelta) -> Self {
        Self {
            baseline: d.baseline,
            comparison: d.comparison,
            absolute_change: d.absolute_change,
            relative_change_pct: d.relative_change_pct,
        }
    }
}

#[pymethods]
impl PyPeriodDelta {
    fn is_growing(&self) -> bool {
        self.absolute_change > 0.0
    }

    fn __repr__(&self) -> String {
        format!(
            "PeriodDelta(baseline={:.2}, comparison={:.2}, absolute_change={:.2})",
            self.baseline, self.comparison, self.absolute_change
        )
    }
}

/// Period-over-period breakdown comparison for one region or device slice.
#[pyclass(name = "SliceComparison")]
pub struct PySliceComparison {
    #[pyo3(get)]
    pub sessions: PyPeriodDelta,
    #[pyo3(get)]
    pub unique_viewers: PyPeriodDelta,
    #[pyo3(get)]
    pub total_watch_seconds: PyPeriodDelta,
    #[pyo3(get)]
    pub avg_watch_seconds: PyPeriodDelta,
}

impl From<core::SliceComparison> for PySliceComparison {
    fn from(c: core::SliceComparison) -> Self {
        Self {
            sessions: PyPeriodDelta::from(&c.sessions),
            unique_viewers: PyPeriodDelta::from(&c.unique_viewers),
            total_watch_seconds: PyPeriodDelta::from(&c.total_watch_seconds),
            avg_watch_seconds: PyPeriodDelta::from(&c.avg_watch_seconds),
        }
    }
}

// ---------------------------------------------------------------------------
// GeoDeviceReport
// ---------------------------------------------------------------------------

/// A full geo/device breakdown snapshot produced by
/// [`PyBreakdownAnalyzer::build_report`].
#[pyclass(name = "GeoDeviceReport")]
pub struct PyGeoDeviceReport {
    inner: core::GeoDeviceReport,
}

#[pymethods]
impl PyGeoDeviceReport {
    #[getter]
    fn total_sessions(&self) -> u64 {
        self.inner.total_sessions
    }

    #[getter]
    fn total_unique_viewers(&self) -> u64 {
        self.inner.total_unique_viewers
    }

    #[getter]
    fn total_watch_seconds(&self) -> f64 {
        self.inner.total_watch_seconds
    }

    #[getter]
    fn overall_avg_watch_seconds(&self) -> f64 {
        self.inner.overall_avg_watch_seconds
    }

    /// `(region_label, metrics)` for every region with sessions.
    fn by_region(&self) -> Vec<(String, PySliceMetrics)> {
        self.inner
            .by_region
            .iter()
            .map(|(r, m)| (r.label().to_string(), PySliceMetrics::from(m)))
            .collect()
    }

    /// `(device_label, metrics)` for every device type with sessions.
    fn by_device(&self) -> Vec<(String, PySliceMetrics)> {
        self.inner
            .by_device
            .iter()
            .map(|(d, m)| (d.label().to_string(), PySliceMetrics::from(m)))
            .collect()
    }

    /// `((region_label, device_label), metrics)` cross-tab.
    fn cross_tab(&self) -> Vec<((String, String), PySliceMetrics)> {
        self.inner
            .cross_tab
            .iter()
            .map(|((r, d), m)| {
                (
                    (r.label().to_string(), d.label().to_string()),
                    PySliceMetrics::from(m),
                )
            })
            .collect()
    }

    /// Fraction of total sessions belonging to `region` (0.0-1.0).
    fn region_share(&self, region: &str) -> PyResult<f64> {
        Ok(self.inner.region_share(parse_region(region)?))
    }

    /// Fraction of total sessions belonging to `device` (0.0-1.0).
    fn device_share(&self, device: &str) -> PyResult<f64> {
        Ok(self.inner.device_share(parse_device(device)?))
    }

    /// Region label with the highest session count, if any.
    fn dominant_region_by_sessions(&self) -> Option<String> {
        self.inner
            .dominant_region_by_sessions()
            .map(|r| r.label().to_string())
    }

    /// Device label with the highest unique-viewer count, if any.
    fn dominant_device_by_viewers(&self) -> Option<String> {
        self.inner
            .dominant_device_by_viewers()
            .map(|d| d.label().to_string())
    }

    fn __repr__(&self) -> String {
        format!(
            "GeoDeviceReport(total_sessions={}, total_unique_viewers={})",
            self.inner.total_sessions, self.inner.total_unique_viewers
        )
    }
}

// ---------------------------------------------------------------------------
// BreakdownAnalyzer
// ---------------------------------------------------------------------------

/// Accumulates session records and computes region/device breakdowns.
#[pyclass(name = "BreakdownAnalyzer")]
#[derive(Default)]
pub struct PyBreakdownAnalyzer {
    inner: core::BreakdownAnalyzer,
}

fn timestamped_records(
    records: Vec<(String, String, String, f64, i64)>,
) -> PyResult<Vec<core::TimestampedRecord>> {
    records
        .into_iter()
        .map(|(viewer_id, region, device, watch_seconds, timestamp_s)| {
            Ok(core::TimestampedRecord::new(
                core::SessionRecord {
                    viewer_id,
                    region: parse_region(&region)?,
                    device: parse_device(&device)?,
                    watch_seconds,
                },
                timestamp_s,
            ))
        })
        .collect()
}

#[pymethods]
impl PyBreakdownAnalyzer {
    #[new]
    fn new() -> Self {
        Self {
            inner: core::BreakdownAnalyzer::new(),
        }
    }

    /// Ingest one session record. `region`/`device` are canonical slugs
    /// (e.g. `"europe"`, `"mobile"`).
    fn ingest(
        &mut self,
        viewer_id: &str,
        region: &str,
        device: &str,
        watch_seconds: f64,
    ) -> PyResult<()> {
        self.inner.ingest(core::SessionRecord {
            viewer_id: viewer_id.to_string(),
            region: parse_region(region)?,
            device: parse_device(device)?,
            watch_seconds,
        });
        Ok(())
    }

    fn session_count(&self) -> usize {
        self.inner.session_count()
    }

    fn breakdown_by_region(&self) -> Vec<(String, PySliceMetrics)> {
        self.inner
            .breakdown_by_region()
            .iter()
            .map(|(r, m)| (r.label().to_string(), PySliceMetrics::from(m)))
            .collect()
    }

    fn breakdown_by_device(&self) -> Vec<(String, PySliceMetrics)> {
        self.inner
            .breakdown_by_device()
            .iter()
            .map(|(d, m)| (d.label().to_string(), PySliceMetrics::from(m)))
            .collect()
    }

    /// Region label with the highest total watch time. Errors if no
    /// sessions have been ingested.
    fn top_region_by_watch_time(&self) -> PyResult<String> {
        self.inner
            .top_region_by_watch_time()
            .map(|r| r.label().to_string())
            .map_err(super::analytics_err)
    }

    /// Device label with the highest session count. Errors if no sessions
    /// have been ingested.
    fn top_device_by_sessions(&self) -> PyResult<String> {
        self.inner
            .top_device_by_sessions()
            .map(|d| d.label().to_string())
            .map_err(super::analytics_err)
    }

    /// Build a full [`PyGeoDeviceReport`] snapshot from the ingested
    /// records.
    fn build_report(&self) -> PyGeoDeviceReport {
        PyGeoDeviceReport {
            inner: self.inner.build_report(),
        }
    }

    /// Compare `region`'s metrics between two periods split at
    /// `split_timestamp_s`. `timestamped` is `(viewer_id, region, device,
    /// watch_seconds, timestamp_s)` — independent of records already
    /// ingested via [`ingest`](Self::ingest).
    fn compare_region_periods(
        &self,
        timestamped: Vec<(String, String, String, f64, i64)>,
        region: &str,
        split_timestamp_s: i64,
    ) -> PyResult<Option<PySliceComparison>> {
        let records = timestamped_records(timestamped)?;
        let region = parse_region(region)?;
        Ok(self
            .inner
            .compare_region_periods(&records, region, split_timestamp_s)
            .map(PySliceComparison::from))
    }

    /// Compare `device`'s metrics between two periods split at
    /// `split_timestamp_s`. See [`compare_region_periods`](Self::compare_region_periods)
    /// for the `timestamped` shape.
    fn compare_device_periods(
        &self,
        timestamped: Vec<(String, String, String, f64, i64)>,
        device: &str,
        split_timestamp_s: i64,
    ) -> PyResult<Option<PySliceComparison>> {
        let records = timestamped_records(timestamped)?;
        let device = parse_device(device)?;
        Ok(self
            .inner
            .compare_device_periods(&records, device, split_timestamp_s)
            .map(PySliceComparison::from))
    }

    fn __repr__(&self) -> String {
        format!(
            "BreakdownAnalyzer(session_count={})",
            self.inner.session_count()
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySliceMetrics>()?;
    m.add_class::<PyPeriodDelta>()?;
    m.add_class::<PySliceComparison>()?;
    m.add_class::<PyGeoDeviceReport>()?;
    m.add_class::<PyBreakdownAnalyzer>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_and_breakdown_by_region() {
        let mut a = PyBreakdownAnalyzer::new();
        a.ingest("v1", "europe", "desktop", 300.0)
            .expect("valid slugs should not error");
        a.ingest("v2", "asia_pacific", "mobile", 200.0)
            .expect("valid slugs should not error");
        assert_eq!(a.session_count(), 2);
        let bd = a.breakdown_by_region();
        assert_eq!(bd.len(), 2);
    }

    #[test]
    fn ingest_rejects_unknown_region() {
        let mut a = PyBreakdownAnalyzer::new();
        let err = a.ingest("v1", "mars", "desktop", 10.0);
        assert!(err.is_err());
    }

    #[test]
    fn top_region_and_device() {
        let mut a = PyBreakdownAnalyzer::new();
        a.ingest("v1", "europe", "desktop", 100.0).expect("ok");
        a.ingest("v2", "asia_pacific", "mobile", 500.0).expect("ok");
        assert_eq!(
            a.top_region_by_watch_time().expect("should succeed"),
            "asia_pacific"
        );
        let report = a.build_report();
        assert_eq!(report.total_sessions(), 2);
        assert_eq!(report.total_unique_viewers(), 2);
    }

    #[test]
    fn empty_analyzer_top_region_errors() {
        let a = PyBreakdownAnalyzer::new();
        assert!(a.top_region_by_watch_time().is_err());
    }

    #[test]
    fn region_share_and_dominant() {
        let mut a = PyBreakdownAnalyzer::new();
        a.ingest("v1", "europe", "desktop", 100.0).expect("ok");
        a.ingest("v2", "europe", "desktop", 100.0).expect("ok");
        a.ingest("v3", "asia_pacific", "mobile", 100.0).expect("ok");
        let report = a.build_report();
        let eu_share = report.region_share("europe").expect("valid region");
        assert!((eu_share - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(
            report.dominant_region_by_sessions(),
            Some("europe".to_string())
        );
    }

    #[test]
    fn compare_region_periods_growth() {
        let a = PyBreakdownAnalyzer::new();
        let records = vec![
            (
                "v1".to_string(),
                "north_america".to_string(),
                "mobile".to_string(),
                60.0,
                50,
            ),
            (
                "v2".to_string(),
                "north_america".to_string(),
                "mobile".to_string(),
                600.0,
                150,
            ),
        ];
        let cmp = a
            .compare_region_periods(records, "north_america", 100)
            .expect("valid slugs")
            .expect("some comparison");
        assert!((cmp.total_watch_seconds.baseline - 60.0).abs() < 1e-9);
        assert!(cmp.total_watch_seconds.is_growing());
    }
}
