// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! `get_reservation_report` - what each project put into a named reservation.
//!
//! An operator tool, run by hand on the cluster. It answers the question the
//! usage reports could not: a project's report says which reservations *it*
//! used, and this says which projects used a *reservation*.
//!
//! It shares the agent's library rather than reimplementing any of it, so that
//! what it says a job consumed is what the agent would say. Requeued attempts,
//! window clipping, node fractions and the expansion factor all come from the
//! same code.
//!
//! ```text
//! get_reservation_report interactive this_month
//! ```
//!
//! What it deliberately does not report is how *full* the reservation was.
//! That is a property of the reservation - its node count and its duration -
//! and job accounting records do not carry it. Every share below is a share of
//! what went in, never of what could have.
//!

// Every dependency of this crate is now declared for the library, which the two
// binaries share; a binary uses only the handful it needs directly. The lint is
// still doing its job on `src/lib.rs`, which is where an unused dependency would
// actually be dead weight.
#![allow(unused_crate_dependencies)]

use std::collections::HashMap;

use anyhow::{Context, Result};

use greatwestern::grammar::{Date, DateRange, ProjectIdentifier};
use greatwestern::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};

use op_slurm::sacctmgr::{record_job, runner, set_commands, ReportTotals};
use op_slurm::slurm::{SlurmJob, SlurmNode, SlurmNodes};

///
/// The node this tool is run on, as the agent's `slurm-default-node` option
/// would describe it.
///
/// Only the *shape* of a node is needed - how much of one a job held is what
/// turns its elapsed time into node-seconds. Override it with `--node` when
/// running somewhere else, or the usage figures will be wrong in proportion to
/// how wrong this is.
///
const DEFAULT_NODE: &str = r#"{ "cpus": 288, "gpus": 4, "mem": 491520, "billing": 864 }"#;

/// A day's query can return a lot of records on a busy cluster, so this is
/// generous compared with the agent's own thirty seconds. A tool run by hand
/// can afford to wait; being told "timed out" is not an answer an operator can
/// use.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Projects shown as their own column in the day-by-day table before the rest
/// are gathered into `other`. Wide enough to see who the reservation is for,
/// narrow enough to stay on a terminal.
const MAX_DAY_TABLE_COLUMNS: usize = 6;

struct Options {
    reservation: String,
    dates: DateRange,
    node: String,
    sacct: String,
    cluster: String,
}

const USAGE: &str = "\
Usage: get_reservation_report <reservation> <period> [options]

  <reservation>   the name of the reservation, as Slurm spells it
  <period>        today, yesterday, this_week, last_week, this_month,
                  last_month, this_year, last_year, a single date
                  (2026-08-01), or a range (2026-08-01:2026-08-14)

Options:
  --node JSON     the shape of a node on this cluster, as the slurm agent's
                  slurm-default-node option gives it. Defaults to the node
                  this tool was built for.
  --sacct CMD     the sacct command to run (default: sacct). Accepts a
                  composite command, e.g. 'docker exec slurmctld sacct'.
  --cluster NAME  restrict the query to one cluster
  --help          show this message

Example:
  get_reservation_report interactive this_month
";

fn parse_args() -> Result<Option<Options>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", USAGE);
        return Ok(None);
    }

    let mut positional: Vec<String> = Vec::new();
    let mut node = DEFAULT_NODE.to_string();
    let mut sacct = "sacct".to_string();
    let mut cluster = String::new();

    let mut remaining = args.into_iter();

    while let Some(arg) = remaining.next() {
        let mut value = |name: &str| -> Result<String> {
            remaining
                .next()
                .with_context(|| format!("{} needs a value. Try --help.", name))
        };

        match arg.as_str() {
            "--node" => node = value("--node")?,
            "--sacct" => sacct = value("--sacct")?,
            "--cluster" => cluster = value("--cluster")?,
            other if other.starts_with('-') => {
                anyhow::bail!("Unknown option '{}'. Try --help.", other);
            }
            other => positional.push(other.to_string()),
        }
    }

    let [reservation, period] = positional.as_slice() else {
        anyhow::bail!(
            "Expected a reservation name and a period, got {} argument(s). Try --help.",
            positional.len()
        );
    };

    let dates = DateRange::parse(period)
        .with_context(|| format!("Could not read '{}' as a period. Try --help.", period))?;

    Ok(Some(Options {
        reservation: reservation.trim().to_string(),
        dates,
        node,
        sacct,
        cluster,
    }))
}

///
/// The OpenPortal project a Slurm account belongs to, if it belongs to one.
///
/// An account this agent manages is named `{portal}.{project}` - the two halves
/// of a `ProjectIdentifier`, which spells them the other way round. Anything
/// that does not fit that shape is an account OpenPortal did not create, and is
/// none of this report's business: it is discarded rather than guessed at.
///
fn project_of_account(account: &str) -> Option<ProjectIdentifier> {
    let (portal, project) = account.trim().split_once('.')?;

    ProjectIdentifier::parse(&format!("{}.{}", project, portal)).ok()
}

///
/// Ask Slurm for every job that ran on one day, for every account.
///
/// Deliberately not filtered by account: the question is who used a
/// reservation, and the answer is not known until the records are read. It is
/// also not filtered by reservation - `sacct` cannot be relied upon to do that
/// across the versions this may meet, and a flag that quietly means something
/// else on an older Slurm is worse than reading a few more records.
///
async fn jobs_on_day(
    day: &Date,
    nodes: &SlurmNodes,
    cluster: &str,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<Vec<SlurmJob>> {
    let start_time = day.day().start_time().and_utc();
    let end_time = day.day().end_time().and_utc();

    if start_time > *now {
        return Ok(Vec::new());
    }

    // never ask for the future - `sacct` is happy to be asked and the clipping
    // would treat the rest of today as consumed
    let end_time = end_time.min(*now);

    // a long expiry: this is a one-shot tool with a person waiting on it, not
    // an agent servicing a job with a deadline
    let expires = *now + chrono::Duration::hours(1);

    let cluster_arg = match cluster.is_empty() {
        true => String::new(),
        false => format!("--cluster={}", cluster),
    };

    let cmd = runner(&expires).await?.build_command(
        "SACCT",
        vec![
            "--noconvert".to_string(),
            "--allocations".to_string(),
            "--allusers".to_string(),
            // one record per attempt - without this everything a requeued job
            // consumed before its final attempt is invisible
            "--duplicates".to_string(),
            format!("--starttime={}", start_time.format("%Y-%m-%dT%H:%M:%S")),
            format!("--endtime={}", end_time.format("%Y-%m-%dT%H:%M:%S")),
            cluster_arg,
            "--json".to_string(),
        ],
    )?;

    let response = runner(&expires)
        .await?
        .run_json(&cmd, QUERY_TIMEOUT)
        .await?;

    Ok(SlurmJob::get_consumers(
        &response,
        &start_time,
        &end_time,
        nodes,
    )?)
}

/// What one day of the reservation came to, once the records are in.
#[derive(Default)]
struct Collected {
    /// project -> its whole report over the period
    projects: HashMap<ProjectIdentifier, ProjectUsageReport>,
    /// accounts seen inside the reservation that OpenPortal does not manage
    unmanaged: HashMap<String, u64>,
    /// jobs that had not finished, so their runtimes are not in the report
    saw_unfinished_job: bool,
    /// the reservation's name as Slurm spells it, which may differ in case
    /// from what was asked for
    name_in_slurm: String,
}

///
/// Read the whole period, one day at a time, keeping only what ran inside the
/// reservation.
///
async fn collect(options: &Options, nodes: &SlurmNodes) -> Result<Collected> {
    let now = chrono::Utc::now();
    let mut collected = Collected {
        name_in_slurm: options.reservation.clone(),
        ..Default::default()
    };

    for day in options.dates.days() {
        let jobs = jobs_on_day(&day, nodes, &options.cluster, &now)
            .await
            .with_context(|| format!("Could not read Slurm accounting for {}", day))?;

        absorb_day(&mut collected, &jobs, &options.reservation, &day);
    }

    Ok(collected)
}

///
/// Fold one day's records into the report, keeping only what ran inside the
/// reservation and belongs to a project OpenPortal manages.
///
/// Split out from the query so that it can be tested against a recorded `sacct`
/// response - which is the half worth testing, the other being a subprocess.
///
fn absorb_day(collected: &mut Collected, jobs: &[SlurmJob], reservation: &str, day: &Date) {
    let start_time = day.day().start_time().and_utc();

    // one report per project per day, so that each day's records are recorded
    // against the window they were queried for - `record_job` counts a job in
    // the window it started in and needs to be told which window that is
    let mut days: HashMap<ProjectIdentifier, (DailyProjectUsageReport, ReportTotals)> =
        HashMap::new();

    for job in jobs {
        if !job.reservation().eq_ignore_ascii_case(reservation) {
            continue;
        }

        // Slurm's spelling wins over the operator's: the two differ only in
        // case, and the report should name the reservation as the cluster does
        collected.name_in_slurm = job.reservation().to_string();

        let Some(project) = project_of_account(job.account()) else {
            *collected
                .unmanaged
                .entry(job.account().to_string())
                .or_default() += 1;
            continue;
        };

        let (report, totals) = days.entry(project).or_default();
        record_job(report, job, &start_time, totals);
    }

    for (project, (report, totals)) in days {
        if totals.saw_unfinished_job() {
            collected.saw_unfinished_job = true;
        }

        collected
            .projects
            .entry(project.clone())
            .or_insert_with(|| ProjectUsageReport::new(&project))
            .set_report(day, &report);
    }
}

/// Projects worst - which is to say largest - first, so the reservation's
/// main occupant is the first thing read.
fn projects_by_usage(collected: &Collected) -> Vec<(&ProjectIdentifier, &ProjectUsageReport)> {
    let mut projects: Vec<(&ProjectIdentifier, &ProjectUsageReport)> =
        collected.projects.iter().collect();

    projects.sort_by(|a, b| {
        b.1.total_usage_including_requeues()
            .seconds()
            .cmp(&a.1.total_usage_including_requeues().seconds())
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });

    projects
}

fn total_usage(collected: &Collected) -> Usage {
    collected
        .projects
        .values()
        .map(|report| report.total_usage_including_requeues())
        .sum()
}

#[tokio::main]
async fn main() -> Result<()> {
    templemeads::config::initialise_tracing();

    let Some(options) = parse_args()? else {
        return Ok(());
    };

    let node = SlurmNode::construct(
        &serde_json::from_str(&options.node)
            .with_context(|| format!("Could not read '{}' as a node", options.node))?,
    )
    .context("Could not read the node description")?;

    let nodes = SlurmNodes::new(&node);

    // one runner: this is a person at a terminal, not an agent under load, and
    // one query at a time keeps the accounting database out of trouble
    set_commands(&options.sacct, "sacctmgr", "scontrol", "scancel", 1).await;

    let collected = collect(&options, &nodes).await?;

    print!("{}", render(&options, &collected));

    Ok(())
}

fn render(options: &Options, collected: &Collected) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let rule = "=".repeat(78);

    let days = options.dates.days();
    let first = days.first().map(|day| day.to_string()).unwrap_or_default();
    let last = days.last().map(|day| day.to_string()).unwrap_or_default();

    // `write!` to a String cannot fail, so the results are discarded rather
    // than unwrapped - `unwrap` is denied in this crate.
    let _ = writeln!(out, "Reservation report for '{}'", collected.name_in_slurm);
    let _ = writeln!(out, "{} to {}", first, last);
    let _ = writeln!(out, "{}", rule);

    if collected.projects.is_empty() {
        let _ = writeln!(
            out,
            "No OpenPortal project ran a job inside this reservation over this period."
        );
        let _ = write!(out, "{}", unmanaged_note(collected));
        let _ = writeln!(out, "{}", rule);
        return out;
    }

    let projects = projects_by_usage(collected);
    let total = total_usage(collected);

    let requeued: Usage = collected
        .projects
        .values()
        .map(|report| report.total_requeue_usage())
        .sum();

    let jobs: u64 = collected.projects.values().fold(0u64, |total, report| {
        total.saturating_add(report.num_jobs())
    });

    let mut users: Vec<String> = collected
        .projects
        .values()
        .flat_map(|report| report.job_users())
        .collect();
    users.sort();
    users.dedup();

    let _ = writeln!(
        out,
        "Consumed inside the reservation : {:>16}",
        total.in_hours().to_string()
    );

    if !requeued.is_zero() {
        let _ = writeln!(
            out,
            "  of which lost to requeues     : {:>16}",
            requeued.in_hours().to_string()
        );
    }

    let _ = writeln!(out, "Jobs started inside it          : {:>16}", jobs);
    let _ = writeln!(
        out,
        "Projects                        : {:>16}",
        projects.len()
    );
    let _ = writeln!(out, "Users                           : {:>16}", users.len());
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "This is what each project put into the reservation, not how full the"
    );
    let _ = writeln!(
        out,
        "reservation was: how much it could have held comes from its node count and"
    );
    let _ = writeln!(
        out,
        "duration, which job accounting records do not carry. Every share below is a"
    );
    let _ = writeln!(out, "share of what went in.");

    if collected.saw_unfinished_job {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Some jobs had not finished when this was read. Their usage so far is"
        );
        let _ = writeln!(
            out,
            "counted; their runtimes and expansion factors are not, and are shown as '-'."
        );
    }

    let _ = write!(out, "{}", unmanaged_note(collected));
    let _ = writeln!(out, "{}", rule);

    for (project, report) in &projects {
        let _ = write!(out, "{}", render_project(project, report, &total));
    }

    let _ = write!(out, "{}", render_day_table(&projects, &options.dates));

    out
}

fn unmanaged_note(collected: &Collected) -> String {
    use std::fmt::Write;

    if collected.unmanaged.is_empty() {
        return String::new();
    }

    let mut accounts: Vec<(&String, &u64)> = collected.unmanaged.iter().collect();
    accounts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    let named: Vec<String> = accounts
        .iter()
        .take(5)
        .map(|(account, records)| format!("{} ({} records)", account, records))
        .collect();

    let mut out = String::new();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} Slurm account(s) in this reservation are not managed by OpenPortal and are",
        accounts.len()
    );
    let _ = writeln!(out, "not reported on: {}.", named.join(", "));

    if accounts.len() > named.len() {
        let _ = writeln!(out, "...and {} more.", accounts.len() - named.len());
    }

    out
}

fn render_project(
    project: &ProjectIdentifier,
    report: &ProjectUsageReport,
    reservation_total: &Usage,
) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let rule = "-".repeat(78);

    let usage = report.total_usage_including_requeues();

    let share = match reservation_total.seconds() {
        0 => 0.0,
        total => 100.0 * usage.seconds() as f64 / total as f64,
    };

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<40} {:>18}  ({:.1}%)",
        project.to_string(),
        usage.in_hours().to_string(),
        share
    );
    let _ = writeln!(out, "{}", rule);

    let _ = writeln!(
        out,
        "{:<16} {:>14} {:>7} {:>11} {:>11} {:>9}",
        "user", "usage", "jobs", "mean wait", "mean run", "expansion"
    );

    let mut users = report.job_users();

    // Everything the user put in, base and requeued together, so this column
    // sums to the project total in the heading above it. A requeued attempt
    // held the reservation's nodes exactly as its replacement did.
    let put_in =
        |user: &str| report.usage_for_local_user(user) + report.requeue_usage_for_local_user(user);

    users.sort_by(|a, b| {
        put_in(b)
            .seconds()
            .cmp(&put_in(a).seconds())
            .then_with(|| a.cmp(b))
    });

    // Zero is this scale's "not recorded" sentinel rather than a score of
    // nought, so it is shown as a dash - a job still running has no runtime
    // and no ratio yet.
    let or_dash = |value: f64| match value > 0.0 {
        true => format!("{:.2}", value),
        false => "-".to_string(),
    };

    let or_dash_hours = |seconds: u64| match seconds > 0 {
        true => Usage::new(seconds).in_hours().to_string(),
        false => "-".to_string(),
    };

    for user in &users {
        let _ = writeln!(
            out,
            "{:<16} {:>14} {:>7} {:>11} {:>11} {:>9}",
            elide(user, 16),
            put_in(user).in_hours().to_string(),
            report.num_jobs_for_user(user),
            or_dash_hours(report.average_wait_seconds_for_user(user)),
            or_dash_hours(report.average_runtime_seconds_for_user(user)),
            or_dash(report.aggregate_expansion_factor_for_user(user)),
        );
    }

    let requeued = report.total_requeue_usage();

    if !requeued.is_zero() || report.num_requeue_events() > 0 {
        let _ = writeln!(
            out,
            "{} requeue event(s) discarded {} inside this reservation.",
            report.num_requeue_events(),
            requeued.in_hours()
        );
    }

    out
}

///
/// A day-by-day table, so that a reservation filling and emptying is visible
/// rather than having to be inferred from one total.
///
fn render_day_table(
    projects: &[(&ProjectIdentifier, &ProjectUsageReport)],
    dates: &DateRange,
) -> String {
    use std::fmt::Write;

    let mut out = String::new();

    let (columns, rest) = projects.split_at(projects.len().min(MAX_DAY_TABLE_COLUMNS));

    let _ = writeln!(out);
    let _ = writeln!(out, "Day by day, in hours");
    let _ = writeln!(out, "{}", "-".repeat(78));

    let mut header = format!("{:<12}", "date");

    for (project, _) in columns {
        let _ = write!(header, " {:>9}", elide(&project.project(), 9));
    }

    if !rest.is_empty() {
        let _ = write!(header, " {:>9}", "other");
    }

    let _ = write!(header, " {:>10}", "total");
    let _ = writeln!(out, "{}", header);

    for day in dates.days() {
        let mut row = format!("{:<12}", day.to_string());
        let mut day_total = Usage::default();

        for (_, report) in columns {
            let usage = usage_on_day(report, &day);
            day_total += usage;
            let _ = write!(row, " {:>9.2}", usage.hours());
        }

        if !rest.is_empty() {
            let other: Usage = rest
                .iter()
                .map(|(_, report)| usage_on_day(report, &day))
                .sum();
            day_total += other;
            let _ = write!(row, " {:>9.2}", other.hours());
        }

        let _ = write!(row, " {:>10.2}", day_total.hours());

        // a reservation with nothing in it on a given day is worth seeing, so
        // empty rows are printed rather than skipped
        let _ = writeln!(out, "{}", row);
    }

    // A totals row, so a column can be checked against the project heading it
    // came from without adding up a month by eye.
    let mut totals = format!("{:<12}", "total");
    let mut whole = Usage::default();

    for (_, report) in columns {
        let usage = report.total_usage_including_requeues();
        whole += usage;
        let _ = write!(totals, " {:>9.2}", usage.hours());
    }

    if !rest.is_empty() {
        let other: Usage = rest
            .iter()
            .map(|(_, report)| report.total_usage_including_requeues())
            .sum();
        whole += other;
        let _ = write!(totals, " {:>9.2}", other.hours());
    }

    let _ = write!(totals, " {:>10.2}", whole.hours());
    let _ = writeln!(out, "{}", "-".repeat(78));
    let _ = writeln!(out, "{}", totals);

    if !rest.is_empty() {
        let _ = writeln!(
            out,
            "'other' is {} further project(s), listed in full above.",
            rest.len()
        );
    }

    let _ = writeln!(out, "{}", "=".repeat(78));

    out
}

fn usage_on_day(report: &ProjectUsageReport, day: &Date) -> Usage {
    report.get_report(day).total_usage_including_requeues()
}

/// Trim a name to fit its column, marking that it was trimmed.
fn elide(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }

    let kept: String = value.chars().take(width.saturating_sub(1)).collect();

    format!("{}~", kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two days the fixture covers.
    const DAY_ONE: i64 = 1772323200;
    const DAY_TWO: i64 = 1772409600;

    fn day(timestamp: i64) -> Date {
        let Some(at) = chrono::DateTime::from_timestamp(timestamp, 0) else {
            unreachable!("the fixture's timestamps are representable");
        };

        Date::from_chrono(&at.date_naive())
    }

    fn test_nodes() -> SlurmNodes {
        let Ok(value) = serde_json::from_str(DEFAULT_NODE) else {
            unreachable!("the built-in node description is JSON");
        };

        let Ok(node) = SlurmNode::construct(&value) else {
            unreachable!("the built-in node description is a node");
        };

        SlurmNodes::new(&node)
    }

    /// The fixture's records, as `sacct` would return them for one day.
    fn jobs_for(day_start: i64) -> Vec<SlurmJob> {
        let Ok(mut response) = serde_json::from_str::<serde_json::Value>(include_str!(
            "../tests/data/sacct-reservation-jobs.json"
        )) else {
            unreachable!("the fixture is JSON");
        };

        let start = day(day_start).day().start_time().and_utc();
        let end = day(day_start).day().end_time().and_utc();

        let Some(records) = response
            .get_mut("jobs")
            .and_then(|jobs| jobs.as_array_mut())
        else {
            unreachable!("the fixture has a jobs array");
        };

        // the overlap filter `--starttime`/`--endtime` applies, so that a test
        // is not handed records the real query could not have seen
        records.retain(|record| {
            let at = |key: &str| {
                record
                    .get("time")
                    .and_then(|time| time.get(key))
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0)
            };

            at("start") < end.timestamp() && at("end") > start.timestamp()
        });

        let Ok(jobs) = SlurmJob::get_consumers(&response, &start, &end, &test_nodes()) else {
            unreachable!("the fixture parses");
        };

        jobs
    }

    fn collected_fixture(reservation: &str) -> Collected {
        let mut collected = Collected {
            name_in_slurm: reservation.to_string(),
            ..Default::default()
        };

        for start in [DAY_ONE, DAY_TWO] {
            absorb_day(&mut collected, &jobs_for(start), reservation, &day(start));
        }

        collected
    }

    #[test]
    fn test_an_account_this_agent_manages_is_read_as_its_project() {
        // Slurm accounts are named portal-first and a ProjectIdentifier is
        // spelled project-first, so the halves swap. Getting this backwards
        // would mislabel every row in the report rather than fail.
        let Some(project) = project_of_account("brics.u6dz") else {
            unreachable!("a managed account resolves");
        };

        assert_eq!(project.project(), "u6dz");
        assert_eq!(project.portal(), "brics");
        assert_eq!(project.to_string(), "u6dz.brics");
    }

    #[test]
    fn test_an_account_this_agent_does_not_manage_is_discarded() {
        // Everything OpenPortal creates is `{portal}.{project}`, so anything
        // else belongs to someone else and is not this report's business. It is
        // dropped rather than guessed at - a site account rendered as a project
        // identifier would be a claim we cannot support.
        assert!(project_of_account("root").is_none());
        assert!(project_of_account("").is_none());
        assert!(project_of_account("brics.u6dz.extra").is_none());
        assert!(project_of_account(".u6dz").is_none());
        assert!(project_of_account("brics.").is_none());
    }

    #[test]
    fn test_only_the_named_reservation_is_reported_on() {
        let collected = collected_fixture("interactive");

        let mut projects: Vec<String> = collected
            .projects
            .keys()
            .map(|project| project.to_string())
            .collect();
        projects.sort();

        assert_eq!(projects, vec!["ab12.brics", "u6dz.brics"]);

        // `gpu_bench` and the unreserved job both belong to u6dz and are both
        // excluded, so its total is the two `interactive` jobs and nothing else
        let Some(u6dz) = collected
            .projects
            .get(&ProjectIdentifier::parse("u6dz.brics").expect("a valid identifier"))
        else {
            unreachable!("u6dz ran in the reservation");
        };

        assert_eq!(u6dz.num_jobs(), 2);

        // 7200s on half a node plus 3600s on a whole one
        assert_eq!(
            u6dz.total_usage_including_requeues(),
            Usage::new(7200 / 2 + 3600)
        );
    }

    #[test]
    fn test_an_unmanaged_account_is_counted_but_not_reported() {
        let collected = collected_fixture("interactive");

        assert_eq!(collected.unmanaged.get("root"), Some(&1));
        assert!(!collected
            .projects
            .keys()
            .any(|project| project.to_string().contains("root")));

        // and it is named in the output, so nobody concludes the reservation
        // held only what is tabulated
        let note = unmanaged_note(&collected);
        assert!(note.contains("root"));
        assert!(note.contains("not managed by OpenPortal"));
    }

    #[test]
    fn test_a_requeued_attempt_counts_towards_what_went_into_the_reservation() {
        // A superseded attempt held the reservation's nodes exactly as its
        // replacement did, so it is part of what the project put in - and is
        // reported separately as well, because it is also what the project
        // lost.
        let collected = collected_fixture("interactive");

        let Some(ab12) = collected
            .projects
            .get(&ProjectIdentifier::parse("ab12.brics").expect("a valid identifier"))
        else {
            unreachable!("ab12 ran in the reservation");
        };

        assert_eq!(ab12.num_requeue_events(), 1);
        assert!(!ab12.total_requeue_usage().is_zero());

        // base plus requeued, and the requeued part is inside it rather than
        // added on top
        assert_eq!(
            ab12.total_usage_including_requeues(),
            ab12.total_usage() + ab12.total_requeue_usage()
        );
    }

    #[test]
    fn test_the_per_user_column_sums_to_the_project_total() {
        // The heading and the table have to agree, or the report invites the
        // reader to work out which of the two is lying.
        let collected = collected_fixture("interactive");

        for (project, report) in &collected.projects {
            let summed: Usage = report
                .job_users()
                .iter()
                .map(|user| {
                    report.usage_for_local_user(user) + report.requeue_usage_for_local_user(user)
                })
                .sum();

            assert_eq!(
                summed,
                report.total_usage_including_requeues(),
                "the user column does not sum to the total for {}",
                project
            );
        }
    }

    #[test]
    fn test_the_report_reads_sensibly_over_the_fixture() {
        let collected = collected_fixture("interactive");

        let Ok(dates) = DateRange::parse("2026-03-01:2026-03-02") else {
            unreachable!("a valid range");
        };

        let options = Options {
            reservation: "interactive".to_string(),
            dates,
            node: DEFAULT_NODE.to_string(),
            sacct: "sacct".to_string(),
            cluster: String::new(),
        };

        let report = render(&options, &collected);

        assert!(report.contains("Reservation report for 'interactive'"));
        assert!(report.contains("u6dz.brics"));
        assert!(report.contains("ab12.brics"));
        assert!(report.contains("user_one"));
        assert!(report.contains("Day by day"));

        // both days appear, including the one only ab12 ran on
        assert!(report.contains("2026-03-01"));
        assert!(report.contains("2026-03-02"));

        // the caveat is not optional - a share of what went in must never be
        // read as a share of what the reservation could have held
        assert!(report.contains("not how full the"));

        // and the account we cannot attribute is declared rather than dropped
        assert!(report.contains("root"));
    }

    #[test]
    fn test_a_reservation_nobody_used_says_so_rather_than_printing_nothing() {
        let collected = collected_fixture("no_such_reservation");

        let Ok(dates) = DateRange::parse("2026-03-01:2026-03-02") else {
            unreachable!("a valid range");
        };

        let options = Options {
            reservation: "no_such_reservation".to_string(),
            dates,
            node: DEFAULT_NODE.to_string(),
            sacct: "sacct".to_string(),
            cluster: String::new(),
        };

        let report = render(&options, &collected);

        assert!(report.contains("No OpenPortal project ran a job inside this reservation"));
        assert!(collected.projects.is_empty());
    }

    #[test]
    fn test_the_reservation_is_matched_however_it_is_capitalised() {
        // An operator types what they remember; Slurm keeps what was created.
        let collected = collected_fixture("INTERACTIVE");

        assert_eq!(collected.projects.len(), 2);

        // and the report names it the way the cluster does
        assert_eq!(collected.name_in_slurm, "interactive");
    }

    #[test]
    fn test_a_long_name_is_trimmed_rather_than_breaking_the_column() {
        assert_eq!(elide("short", 9), "short");
        assert_eq!(elide("exactlynine", 11), "exactlynine");
        assert_eq!(elide("averylongprojectname", 9), "averylon~");
    }
}
