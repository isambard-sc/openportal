// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Turns whatever cost-report JSON files the cloud operators have dropped
//! into the accounting directory into a `ProjectUsageReport`.
//!
//! See `docs/plans/archive/op-cloudaccount-design.md` §6 for the full algorithm
//! this implements: parse tolerantly, group by project, sort and dedup by
//! `generated_at`, compute deltas between consecutive cumulative reports,
//! and spread each delta evenly across the calendar days it spans.
//!
//! `Usage` (a `u64` count of "seconds") is reinterpreted here as
//! micro-currency-units: 1 `Usage` second = 1e-6 of the configured
//! currency (see design doc §7 for why).

use chrono::{DateTime, Days, NaiveDate, Utc};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Maximum number of calendar days a single cost-report window may span.
///
/// `time_period.start` is a plain `NaiveDate` deserialised from an operator-written
/// file, and chrono accepts the full range - so `"start":"-262143-01-01"` yields
/// ~191 million `NaiveDate`s and then a `DailyProjectUsageReport` (four `HashMap`s)
/// for each. A plausible *typo* - `0001-01-01` for `2001-01-01` - already produces
/// ~739,000 daily reports, which are then serialised into a Job result and pushed
/// over the wire. Roughly five years is far beyond any real billing window. See
/// `docs/specifications/security-review-2.md` (finding R26).
const MAX_REPORT_WINDOW_DAYS: i64 = 5 * 366;

/// Maximum size of a single cost-report file. `read_to_string` had no cap, and
/// neither `read_dir` nor `read_to_string` checked for a regular file, so a
/// multi-gigabyte file - or a symlink to `/dev/zero` - was read entirely into
/// memory. See finding R26.
const MAX_REPORT_FILE_BYTES: u64 = 8 * 1024 * 1024;

use greatwestern::grammar::{Date, DateRange, ProjectIdentifier};
use greatwestern::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};
use serde::Deserialize;
use templemeads::Error;

/// 1 `Usage` second = 1 / CURRENCY_SCALE of the configured currency.
/// See design doc §7 - this gives ~18 trillion dollars of headroom in a
/// u64 and resolves to a hundredth of a hundredth of a cent, which is far
/// finer than needed given this is accounting, not billing.
const CURRENCY_SCALE: f64 = 1_000_000.0;

#[derive(Debug, Clone, Deserialize)]
struct TimePeriod {
    start: NaiveDate,
    end: NaiveDate,
}

/// One line of the per-service cost breakdown in `details[]`, e.g.
/// `{"service_key": "Amazon Simple Storage Service", "amount": 0.0008786623, ...}`.
/// `component`/`unit`/`group_keys`/`time_period` are cloud-specific or redundant
/// with the report-level fields and are ignored.
#[derive(Debug, Clone, Deserialize)]
struct CostDetailLine {
    service_key: String,
    amount: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct CostReportFile {
    project: String,
    generated_at: DateTime<Utc>,
    #[serde(default)]
    currency: Option<String>,
    time_period: TimePeriod,
    total: f64,
    #[serde(default)]
    components: HashMap<String, f64>,
    #[serde(default)]
    details: Vec<CostDetailLine>,
    #[serde(default)]
    allocated_budget: Option<f64>,
}

#[derive(Debug, Clone)]
struct ParsedReport {
    generated_at: DateTime<Utc>,
    period_start: NaiveDate,
    /// Last day actually covered by `time_period` - already converted from the
    /// half-open `end` the operator's file reports, see `parse_report`.
    period_end: NaiveDate,
    total: f64,
    components: HashMap<String, f64>,
    allocated_budget: Option<f64>,
}

struct Config {
    accounting_dir: Option<PathBuf>,
    currency: String,
}

static CONFIG: Lazy<RwLock<Config>> = Lazy::new(|| {
    RwLock::new(Config {
        accounting_dir: None,
        currency: "USD".to_string(),
    })
});

type Fingerprint = Vec<(String, Option<SystemTime>, u64)>;

struct ReportCache {
    fingerprint: Fingerprint,
    reports: HashMap<ProjectIdentifier, ProjectUsageReport>,
}

static CACHE: Lazy<RwLock<Option<ReportCache>>> = Lazy::new(|| RwLock::new(None));

pub async fn initialise(accounting_dir: &Path, currency: &str) -> Result<(), Error> {
    tokio::fs::create_dir_all(accounting_dir)
        .await
        .map_err(|e| {
            Error::Failed(format!(
                "Cannot create cloudaccount accounting-dir '{}': {}",
                accounting_dir.display(),
                e
            ))
        })?;

    let mut config = CONFIG.write().await;
    config.accounting_dir = Some(accounting_dir.to_path_buf());
    config.currency = currency.to_string();

    Ok(())
}

async fn accounting_dir() -> Result<PathBuf, Error> {
    CONFIG.read().await.accounting_dir.clone().ok_or_else(|| {
        Error::Misconfigured(
            "cloudaccount accounting directory has not been initialised".to_string(),
        )
    })
}

/// List every `*.json` file in the accounting directory along with its
/// modification time and size - used as a cheap fingerprint to decide
/// whether the cached reports are still up to date.
async fn list_fingerprint(dir: &Path) -> Result<Fingerprint, Error> {
    let mut fingerprint = Vec::new();

    let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot read cloudaccount accounting-dir '{}': {}",
            dir.display(),
            e
        ))
    })?;

    while let Some(entry) = entries.next_entry().await.map_err(Error::IO)? {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let metadata = entry.metadata().await.map_err(Error::IO)?;

        fingerprint.push((
            path.display().to_string(),
            metadata.modified().ok(),
            metadata.len(),
        ));
    }

    fingerprint.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(fingerprint)
}

fn parse_report(
    path: &Path,
    contents: &str,
    currency: &str,
) -> Option<(ProjectIdentifier, ParsedReport)> {
    let raw: CostReportFile = match serde_json::from_str(contents) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(
                "Could not parse cost-report file '{}': {}. Skipping.",
                path.display(),
                e
            );
            return None;
        }
    };

    let project = match ProjectIdentifier::parse(&raw.project) {
        Ok(project) => project,
        Err(e) => {
            tracing::warn!(
                "Cost-report file '{}' has an invalid project '{}': {}. Skipping.",
                path.display(),
                raw.project,
                e
            );
            return None;
        }
    };

    if let Some(reported_currency) = &raw.currency {
        if !reported_currency.eq_ignore_ascii_case(currency) {
            tracing::warn!(
                "Cost-report file '{}' is in currency '{}' but this cloud account is \
                 configured for '{}'. Using the value as-is - no FX conversion is applied.",
                path.display(),
                reported_currency,
                currency
            );
        }
    }

    // `time_period` follows AWS Cost Explorer's convention: `end` is half-open, the
    // first day *not* covered by the period (`{"start":"2026-06-01","end":"2026-07-01"}`
    // describes June only). `ParsedReport::period_end` stores the last day actually
    // covered so every other use of it can treat it as an ordinary inclusive endpoint.
    let period_end = raw
        .time_period
        .end
        .checked_sub_days(Days::new(1))
        .unwrap_or(raw.time_period.end);

    // Prefer the per-service breakdown in `details[]` (keyed by `service_key`, e.g.
    // "Amazon Simple Storage Service") over the coarse `components` map, which the
    // example payload buckets everything into a single "other" entry under. Fall
    // back to `components` for operators who haven't started sending `details` yet.
    // Zero-amount lines (most AWS services, on any given day) are dropped rather
    // than carried through as a component reporting no usage.
    let components = if raw.details.is_empty() {
        raw.components
    } else {
        let mut components: HashMap<String, f64> = HashMap::new();
        for detail in raw.details {
            if detail.amount != 0.0 {
                *components.entry(detail.service_key).or_insert(0.0) += detail.amount;
            }
        }
        components
    };

    Some((
        project,
        ParsedReport {
            generated_at: raw.generated_at,
            period_start: raw.time_period.start,
            period_end,
            total: raw.total,
            components,
            allocated_budget: raw.allocated_budget,
        },
    ))
}

/// Keep only the reports for `project`, deduped by `generated_at` (keeping
/// the larger `total` on a genuine conflict - design doc §6.3) and sorted
/// by `generated_at` (the trusted ordering signal - design doc §6.2). Pure
/// and separated from the file-reading in `load_sorted_reports` so the
/// dedup/ordering logic can be unit tested without touching the filesystem.
fn dedupe_and_sort(
    parsed: Vec<(ProjectIdentifier, ParsedReport)>,
    project: &ProjectIdentifier,
) -> Vec<ParsedReport> {
    let mut by_timestamp: HashMap<DateTime<Utc>, ParsedReport> = HashMap::new();

    for (report_project, parsed) in parsed {
        if &report_project != project {
            continue;
        }

        match by_timestamp.get(&parsed.generated_at) {
            Some(existing) if existing.total >= parsed.total => {
                tracing::warn!(
                    "Duplicate cost-report for project {} at {} - keeping the one with the larger total.",
                    project,
                    parsed.generated_at
                );
            }
            _ => {
                by_timestamp.insert(parsed.generated_at, parsed);
            }
        }
    }

    let mut reports: Vec<ParsedReport> = by_timestamp.into_values().collect();
    reports.sort_by_key(|r| r.generated_at);

    reports
}

/// Read every cost-report file in the accounting directory, keeping only
/// the ones for `project`, deduped and sorted by `generated_at`.
async fn load_sorted_reports(project: &ProjectIdentifier) -> Result<Vec<ParsedReport>, Error> {
    let dir = accounting_dir().await?;
    let currency = CONFIG.read().await.currency.clone();

    let mut parsed = Vec::new();

    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot read cloudaccount accounting-dir '{}': {}",
            dir.display(),
            e
        ))
    })?;

    while let Some(entry) = entries.next_entry().await.map_err(Error::IO)? {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        // `symlink_metadata` rather than `metadata`, so a symlink is judged as a
        // symlink rather than followed to whatever it points at - a link to
        // `/dev/zero` is not a regular file and has no meaningful length. See
        // finding R26.
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if !metadata.is_file() => {
                tracing::warn!(
                    "Skipping cost-report entry '{}': it is not a regular file.",
                    path.display()
                );
                continue;
            }
            Ok(metadata) if metadata.len() > MAX_REPORT_FILE_BYTES => {
                tracing::warn!(
                    "Skipping cost-report file '{}': it is {} bytes, more than the \
                     {} byte maximum.",
                    path.display(),
                    metadata.len(),
                    MAX_REPORT_FILE_BYTES
                );
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    "Could not stat cost-report file '{}': {}. Skipping.",
                    path.display(),
                    e
                );
                continue;
            }
        }

        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(e) => {
                tracing::warn!(
                    "Could not read cost-report file '{}': {}. Skipping.",
                    path.display(),
                    e
                );
                continue;
            }
        };

        if let Some(pair) = parse_report(&path, &contents, &currency) {
            parsed.push(pair);
        }
    }

    Ok(dedupe_and_sort(parsed, project))
}

/// Every calendar day touched by the half-open-in-spirit window
/// `[start, end]` (both inclusive, since `end` is a point in time that
/// falls somewhere within its day).
fn days_touched(start: NaiveDate, end: NaiveDate) -> Vec<NaiveDate> {
    let mut days = Vec::new();
    let mut current = start.min(end);
    let end = start.max(end);

    // Hard cap as well as the window clamp in `clamp_window`, so this function is
    // safe to call with any pair of dates - it allocates one `NaiveDate` plus one
    // `DailyProjectUsageReport` per day, and both ends come from a file. See
    // finding R26.
    loop {
        days.push(current);

        if current >= end {
            break;
        }

        if days.len() as i64 >= MAX_REPORT_WINDOW_DAYS {
            tracing::warn!(
                "Cost-report window {} .. {} spans more than the {} day maximum - \
                 truncating at {}.",
                start,
                end,
                MAX_REPORT_WINDOW_DAYS,
                current
            );
            break;
        }

        current = match current.checked_add_days(Days::new(1)) {
            Some(next) => next,
            None => break,
        };
    }

    days
}

/// Clamp a report window to at most [`MAX_REPORT_WINDOW_DAYS`] ending at `end`, and
/// reject a window whose `end` is in the future.
///
/// Applied to the window *before* it reaches `days_touched`, so an absurd
/// `period_start` produces a short window rather than a truncated giant one. See
/// finding R26.
fn clamp_window(start: NaiveDate, end: NaiveDate) -> (NaiveDate, NaiveDate) {
    let earliest = end
        .checked_sub_days(Days::new(MAX_REPORT_WINDOW_DAYS as u64))
        .unwrap_or(end);

    if start < earliest {
        tracing::warn!(
            "Cost-report window starts at {}, more than {} days before {} - \
             clamping the start to {}. Check the report's time_period.start.",
            start,
            MAX_REPORT_WINDOW_DAYS,
            end,
            earliest
        );

        return (earliest, end);
    }

    (start, end)
}

fn to_usage(amount: f64) -> Usage {
    let scaled = amount * CURRENCY_SCALE;

    if !scaled.is_finite() || scaled <= 0.0 {
        return Usage::default();
    }

    Usage::new(scaled.round() as u64)
}

/// Spread `total_delta`/`component_deltas`, accrued over the window
/// `[window_start, window_end]`, evenly across the calendar days that
/// window touches - design doc §6.5.
fn spread_across_days(
    daily: &mut HashMap<Date, DailyProjectUsageReport>,
    window_start: NaiveDate,
    window_end: NaiveDate,
    total_delta: f64,
    component_deltas: &HashMap<String, f64>,
) {
    let days = days_touched(window_start, window_end);
    let n = days.len() as f64;

    if n == 0.0 {
        return;
    }

    let total_share = to_usage(total_delta / n);
    let component_shares: HashMap<&String, Usage> = component_deltas
        .iter()
        .map(|(component, amount)| (component, to_usage(amount / n)))
        .collect();

    for day in days {
        let date = Date::from_chrono(&day);
        let report = daily.entry(date).or_default();

        report.add_unattributed_usage(total_share);

        for (component, share) in &component_shares {
            report.add_unattributed_component_usage(component, *share);
        }
    }
}

/// Clamp a cumulative-total difference to zero, warning if it was
/// negative (a credit/refund/correction dipped the cumulative total) -
/// design doc §6.4.
fn clamped_delta(current: f64, previous: f64, project: &ProjectIdentifier, what: &str) -> f64 {
    let delta = current - previous;

    if delta < 0.0 {
        tracing::warn!(
            "Cost report for project {} shows '{}' decreasing ({} -> {}) - treating the delta \
             as 0 rather than negative. This can happen with credits/refunds/corrections.",
            project,
            what,
            previous,
            current
        );
        0.0
    } else {
        delta
    }
}

fn reconstruct(project: &ProjectIdentifier, reports: &[ParsedReport]) -> ProjectUsageReport {
    let mut usage_report = ProjectUsageReport::new(project);

    if reports.is_empty() {
        return usage_report;
    }

    let mut daily: HashMap<Date, DailyProjectUsageReport> = HashMap::new();
    let mut previous: Option<&ParsedReport> = None;

    for report in reports {
        let (window_start, total_delta, component_deltas) = match previous {
            None => (report.period_start, report.total, report.components.clone()),
            Some(previous) => {
                let total_delta = clamped_delta(report.total, previous.total, project, "total");

                let mut component_deltas = HashMap::new();
                let mut keys: std::collections::HashSet<&String> =
                    report.components.keys().collect();
                keys.extend(previous.components.keys());

                for key in keys {
                    let current_value = report.components.get(key).copied().unwrap_or(0.0);
                    let previous_value = previous.components.get(key).copied().unwrap_or(0.0);
                    component_deltas.insert(
                        key.clone(),
                        clamped_delta(current_value, previous_value, project, key),
                    );
                }

                (
                    previous.generated_at.date_naive(),
                    total_delta,
                    component_deltas,
                )
            }
        };

        // `generated_at` is when the report was exported, not necessarily when the
        // period it describes closed - an operator's export script can run well
        // after `time_period.end`, and no further cost accrues in that gap. Cap the
        // window there so a late export doesn't smear its delta onto days the
        // report never covered.
        let (window_start, window_end) = clamp_window(
            window_start,
            report.generated_at.date_naive().min(report.period_end),
        );

        spread_across_days(
            &mut daily,
            window_start,
            window_end,
            total_delta,
            &component_deltas,
        );

        previous = Some(report);
    }

    // A day is only "complete" once it's fully behind the most recent
    // report we have - the day the latest report landed on could still
    // be revised by the next drop. Design doc §6.6.
    let last_report_date = reports
        .last()
        .map(|r| r.generated_at.date_naive())
        .unwrap_or_else(|| Utc::now().date_naive());

    for (date, mut report) in daily {
        if date.to_chrono() < last_report_date {
            report.set_complete();
        }

        usage_report.set_report(&date, &report);
    }

    usage_report
}

/// Build the full `ProjectUsageReport` for `project` from whatever cost
/// files are in the accounting directory, restricted to `dates`.
/// Refuse to answer for a project that has not been assigned to this cloud
/// account.
///
/// `GetProjects` filters by portal, but `GetUsageReport` and `GetLimit` took the
/// project straight from the Job and scanned every report file for it - so a peer
/// authorised for portal A could read the cost history of `someproject.portalB`
/// from this account. Gated here rather than at the instruction handlers so no
/// future caller can bypass it; `GetUsageReports` passes projects that already come
/// from the assignment state, so it is unaffected. See
/// `docs/specifications/security-review-2.md` (finding R30).
async fn assert_project_is_assigned(project: &ProjectIdentifier) -> Result<(), Error> {
    match crate::state::get_project_mapping(project).await {
        Ok(_) => Ok(()),
        Err(_) => {
            tracing::warn!(
                "Refusing to report on project '{}': it has not been assigned to this \
                 cloud account.",
                project
            );

            Err(Error::NotFound(format!(
                "Project '{}' has not been assigned to this cloud account",
                project
            )))
        }
    }
}

pub async fn get_usage_report(
    project: &ProjectIdentifier,
    dates: &DateRange,
) -> Result<ProjectUsageReport, Error> {
    assert_project_is_assigned(project).await?;

    let dir = accounting_dir().await?;
    let fingerprint = list_fingerprint(&dir).await?;

    {
        let cache = CACHE.read().await;
        if let Some(cache) = cache.as_ref() {
            if cache.fingerprint == fingerprint {
                if let Some(report) = cache.reports.get(project) {
                    return Ok(report.filter(dates));
                }
            }
        }
    }

    let reports = load_sorted_reports(project).await?;
    let report = reconstruct(project, &reports);

    let mut cache = CACHE.write().await;
    match cache.as_mut() {
        Some(cache) if cache.fingerprint == fingerprint => {
            cache.reports.insert(project.clone(), report.clone());
        }
        _ => {
            let mut reports = HashMap::new();
            reports.insert(project.clone(), report.clone());
            *cache = Some(ReportCache {
                fingerprint,
                reports,
            });
        }
    }

    Ok(report.filter(dates))
}

/// The most recently reported `allocated_budget` for `project`, or a zero
/// `Usage` if there are no cost reports for it yet.
pub async fn get_limit(project: &ProjectIdentifier) -> Result<Usage, Error> {
    assert_project_is_assigned(project).await?;

    let reports = load_sorted_reports(project).await?;

    Ok(reports
        .last()
        .and_then(|r| r.allocated_budget)
        .map(to_usage)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::path::Path;

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn test_an_absurd_report_window_cannot_exhaust_memory() {
        // `time_period.start` is a plain NaiveDate from an operator-written file and
        // chrono accepts the full range. `days_touched` allocates a NaiveDate and
        // then a DailyProjectUsageReport (four HashMaps) per day, so
        // "-262143-01-01" meant ~191 million of each. A plausible *typo* -
        // 0001-01-01 for 2001-01-01 - already meant ~739,000. See finding R26.
        let end = nd("2026-08-03");

        let (start, _) = clamp_window(NaiveDate::MIN, end);
        assert!(
            (end - start).num_days() <= MAX_REPORT_WINDOW_DAYS,
            "an absurd start must be clamped, got a {} day window",
            (end - start).num_days()
        );

        // the realistic typo case
        let (start, _) = clamp_window(nd("0001-01-01"), end);
        assert!((end - start).num_days() <= MAX_REPORT_WINDOW_DAYS);

        // A normal window is untouched.
        let (start, kept_end) = clamp_window(nd("2026-07-01"), end);
        assert_eq!(start, nd("2026-07-01"));
        assert_eq!(kept_end, end);

        // ...and days_touched is independently capped, so it is safe for any input.
        assert!(
            days_touched(NaiveDate::MIN, NaiveDate::MAX).len() as i64 <= MAX_REPORT_WINDOW_DAYS
        );

        // Ordinary spans still enumerate exactly.
        assert_eq!(days_touched(nd("2026-07-01"), nd("2026-07-03")).len(), 3);
        assert_eq!(days_touched(nd("2026-07-01"), nd("2026-07-01")).len(), 1);
    }

    fn nd(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn report(generated_at: &str, period_start: &str, total: f64) -> ParsedReport {
        ParsedReport {
            generated_at: dt(generated_at),
            period_start: nd(period_start),
            // Far beyond any test's `generated_at`, so it never clamps unless a
            // test overrides it via `..report(...)` - see
            // `test_reconstruct_late_export_does_not_smear_past_period_end`.
            period_end: NaiveDate::MAX,
            total,
            components: HashMap::new(),
            allocated_budget: None,
        }
    }

    #[test]
    fn test_days_touched_single_day() {
        let d = nd("2026-06-01");
        assert_eq!(days_touched(d, d), vec![d]);
    }

    #[test]
    fn test_days_touched_spans_multiple_days() {
        let start = nd("2026-06-01");
        let end = nd("2026-06-03");
        assert_eq!(
            days_touched(start, end),
            vec![nd("2026-06-01"), nd("2026-06-02"), nd("2026-06-03")]
        );
    }

    #[test]
    fn test_to_usage_scales_by_currency_scale() {
        assert_eq!(to_usage(1.5).seconds(), 1_500_000);
    }

    #[test]
    fn test_to_usage_clamps_non_positive_to_zero() {
        assert_eq!(to_usage(-5.0).seconds(), 0);
        assert_eq!(to_usage(0.0).seconds(), 0);
    }

    #[test]
    fn test_dedupe_and_sort_keeps_larger_total_on_conflict() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let low = report("2026-06-01T00:00:00Z", "2026-06-01", 5.0);
        let high = report("2026-06-01T00:00:00Z", "2026-06-01", 9.0);

        let result = dedupe_and_sort(
            vec![(project.clone(), low), (project.clone(), high)],
            &project,
        );

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].total, 9.0);
    }

    #[test]
    fn test_dedupe_and_sort_filters_other_projects_and_sorts_by_time() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let other = ProjectIdentifier::parse("other.portal").unwrap();

        let later = report("2026-06-02T00:00:00Z", "2026-06-01", 20.0);
        let earlier = report("2026-06-01T00:00:00Z", "2026-06-01", 10.0);
        let not_ours = report("2026-06-01T00:00:00Z", "2026-06-01", 999.0);

        let result = dedupe_and_sort(
            vec![
                (project.clone(), later),
                (project.clone(), earlier),
                (other, not_ours),
            ],
            &project,
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].total, 10.0);
        assert_eq!(result[1].total, 20.0);
    }

    #[test]
    fn test_cost_report_file_tolerates_missing_optional_and_unknown_fields() {
        let json = r#"{
            "project": "myproject.waldur",
            "generated_at": "2026-07-16T21:03:35.242832+00:00",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 0.0009,
            "components": {"other": 0.0009},
            "sandbox_lease": {"status": "Active"},
            "unexpected_future_field": 42
        }"#;

        let parsed: CostReportFile = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.total, 0.0009);
        assert!(parsed.currency.is_none());
        assert!(parsed.allocated_budget.is_none());
    }

    #[test]
    fn test_parse_report_succeeds_despite_currency_mismatch() {
        let json = r#"{
            "project": "myproject.waldur",
            "generated_at": "2026-07-16T21:03:35+00:00",
            "currency": "GBP",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 1.5,
            "components": {}
        }"#;

        assert!(parse_report(Path::new("test.json"), json, "USD").is_some());
    }

    #[test]
    fn test_parse_report_converts_half_open_end_to_last_covered_day() {
        let json = r#"{
            "project": "myproject.waldur",
            "generated_at": "2026-07-16T21:03:35+00:00",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 0.0009,
            "components": {}
        }"#;

        let (_, parsed) = parse_report(Path::new("test.json"), json, "USD").unwrap();

        // "end": "2026-07-01" is AWS's half-open convention - the period is June
        // only, so the last day actually covered is 2026-06-30, not 2026-07-01.
        assert_eq!(parsed.period_end, nd("2026-06-30"));
    }

    #[test]
    fn test_parse_report_uses_service_key_from_details_over_flat_components() {
        let json = r#"{
            "project": "myproject.waldur",
            "generated_at": "2026-07-16T21:03:35+00:00",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 0.0009,
            "components": {"other": 0.0009},
            "details": [
                {"service_key": "Amazon Simple Storage Service", "component": "other", "amount": 0.0008786623, "unit": "USD"},
                {"service_key": "AWS Secrets Manager", "component": "other", "amount": 5.06e-05, "unit": "USD"},
                {"service_key": "AWS CloudFormation", "component": "other", "amount": 0.0, "unit": "USD"}
            ]
        }"#;

        let (_, parsed) = parse_report(Path::new("test.json"), json, "USD").unwrap();

        assert_eq!(
            parsed.components.get("Amazon Simple Storage Service"),
            Some(&0.0008786623)
        );
        assert_eq!(
            parsed.components.get("AWS Secrets Manager"),
            Some(&5.06e-05)
        );
        // zero-amount lines are dropped rather than carried through as a
        // component reporting no usage.
        assert!(!parsed.components.contains_key("AWS CloudFormation"));
        // the flat map is ignored once `details` is present.
        assert!(!parsed.components.contains_key("other"));
    }

    #[test]
    fn test_parse_report_sums_duplicate_service_keys_in_details() {
        let json = r#"{
            "project": "myproject.waldur",
            "generated_at": "2026-07-16T21:03:35+00:00",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 1.5,
            "details": [
                {"service_key": "Amazon Simple Storage Service", "component": "other", "amount": 1.0},
                {"service_key": "Amazon Simple Storage Service", "component": "other", "amount": 0.5}
            ]
        }"#;

        let (_, parsed) = parse_report(Path::new("test.json"), json, "USD").unwrap();

        assert_eq!(
            parsed.components.get("Amazon Simple Storage Service"),
            Some(&1.5)
        );
    }

    #[test]
    fn test_parse_report_falls_back_to_flat_components_without_details() {
        let json = r#"{
            "project": "myproject.waldur",
            "generated_at": "2026-07-16T21:03:35+00:00",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 0.0009,
            "components": {"other": 0.0009}
        }"#;

        let (_, parsed) = parse_report(Path::new("test.json"), json, "USD").unwrap();

        assert_eq!(parsed.components.get("other"), Some(&0.0009));
    }

    #[test]
    fn test_parse_report_skips_invalid_project() {
        let json = r#"{
            "project": "not-a-valid-project-identifier",
            "generated_at": "2026-07-16T21:03:35+00:00",
            "time_period": {"start": "2026-06-01", "end": "2026-07-01"},
            "total": 1.5,
            "components": {}
        }"#;

        assert!(parse_report(Path::new("test.json"), json, "USD").is_none());
    }

    #[test]
    fn test_reconstruct_first_report_spreads_from_period_start() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let reports = vec![report("2026-06-03T12:00:00Z", "2026-06-01", 30.0)];

        let usage_report = reconstruct(&project, &reports);

        // window is 2026-06-01..=2026-06-03 (3 days), delta 30 -> 10/day
        for day in ["2026-06-01", "2026-06-02", "2026-06-03"] {
            let date = Date::from_chrono(&nd(day));
            assert_eq!(
                usage_report.get_report(&date).total_usage().seconds(),
                10 * 1_000_000
            );
        }
    }

    #[test]
    fn test_reconstruct_late_export_does_not_smear_past_period_end() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();

        // time_period is 2026-06-01..=2026-06-30 (30 days) but the export ran
        // more than two weeks later - the delta must spread across the 30 days
        // the period actually covers, not the 46 days up to generated_at.
        let reports = vec![ParsedReport {
            period_end: nd("2026-06-30"),
            ..report("2026-07-16T21:03:35Z", "2026-06-01", 30.0)
        }];

        let usage_report = reconstruct(&project, &reports);

        let in_period = Date::from_chrono(&nd("2026-06-15"));
        assert_eq!(
            usage_report.get_report(&in_period).total_usage().seconds(),
            1_000_000 // 30 / 30 days
        );

        let after_period = Date::from_chrono(&nd("2026-07-14"));
        assert_eq!(
            usage_report
                .get_report(&after_period)
                .total_usage()
                .seconds(),
            0
        );
    }

    #[test]
    fn test_reconstruct_negative_delta_clamped_to_zero() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let reports = vec![
            report("2026-06-01T00:00:00Z", "2026-06-01", 50.0),
            report("2026-06-02T00:00:00Z", "2026-06-01", 40.0), // cumulative total dipped (credit/refund)
        ];

        let usage_report = reconstruct(&project, &reports);

        let day2 = Date::from_chrono(&nd("2026-06-02"));
        assert_eq!(usage_report.get_report(&day2).total_usage().seconds(), 0);
    }

    #[test]
    fn test_reconstruct_marks_only_days_before_last_report_complete() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let reports = vec![
            report("2026-06-01T00:00:00Z", "2026-06-01", 10.0),
            report("2026-06-03T00:00:00Z", "2026-06-01", 40.0),
        ];

        let usage_report = reconstruct(&project, &reports);

        let day1 = Date::from_chrono(&nd("2026-06-01"));
        let day2 = Date::from_chrono(&nd("2026-06-02"));
        let day3 = Date::from_chrono(&nd("2026-06-03"));

        assert!(usage_report.get_report(&day1).is_complete());
        assert!(usage_report.get_report(&day2).is_complete());
        assert!(!usage_report.get_report(&day3).is_complete());
    }

    #[test]
    fn test_reconstruct_missing_component_treated_as_zero() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();

        let mut first_components = HashMap::new();
        first_components.insert("cpu".to_string(), 10.0);

        let mut second_components = HashMap::new();
        second_components.insert("cpu".to_string(), 25.0);
        second_components.insert("gpu".to_string(), 5.0); // new component appears

        let reports = vec![
            ParsedReport {
                components: first_components,
                ..report("2026-06-01T00:00:00Z", "2026-06-01", 10.0)
            },
            ParsedReport {
                components: second_components,
                ..report("2026-06-01T12:00:00Z", "2026-06-01", 40.0)
            },
        ];

        let usage_report = reconstruct(&project, &reports);
        let day = Date::from_chrono(&nd("2026-06-01"));

        // cpu: 10 (first report's own value, no predecessor) + 15 (25 - 10 delta) = 25
        // gpu: 0 (missing from first report, treated as 0) + 5 (5 - 0 delta) = 5
        assert_eq!(
            usage_report
                .get_component("cpu")
                .get_report(&day)
                .total_usage()
                .seconds(),
            25 * 1_000_000
        );
        assert_eq!(
            usage_report
                .get_component("gpu")
                .get_report(&day)
                .total_usage()
                .seconds(),
            5 * 1_000_000
        );
    }
}
