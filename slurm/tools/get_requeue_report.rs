// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! `get_requeue_report` - what requeueing cost a project, or the whole cluster.
//!
//! An operator tool, run by hand on the cluster. It answers two questions that
//! became worth asking when requeues stopped being invisible and started being
//! charged for: what has this project lost to requeues, and - the one no
//! per-project report can answer - which projects are losing the most, and is
//! the cluster as a whole losing more than it used to.
//!
//! It shares the agent's library rather than reimplementing any of it, so what
//! it says a job consumed is what the agent would say. The charged/absorbed
//! split in particular comes from the same `RequeuePolicy`, so a project
//! disputing a charge and the agent that levied it are reading one rule.
//!
//! ```text
//! get_requeue_report myproject.brics this_month
//! get_requeue_report --cluster-wide yesterday
//! ```
//!
//! Cluster-wide mode reads *every* job on the machine for each day in the
//! period, which is the heaviest thing anything here asks of `sacct`. Keep the
//! period short - though "short" turns out to be more generous than expected:
//! a whole month, cluster-wide, has been run on a production machine with
//! several hundred projects and half a million jobs, and came back with a
//! report. The warning above that many days stands as advice rather than as a
//! prediction of failure.
//!

// Every dependency of this crate is declared for the library, which the
// binaries share; a binary uses only the handful it needs directly. The lint is
// still doing its job on `src/lib.rs`, which is where an unused dependency would
// actually be dead weight.
#![allow(unused_crate_dependencies)]

use std::collections::HashMap;
use std::fmt::Write;

use anyhow::{Context, Result};

use greatwestern::grammar::{Date, DateRange, ProjectIdentifier};
use greatwestern::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};

use op_slurm::dayquery::{jobs_on_day, DayRecords, JobQuery};
use op_slurm::sacctmgr::{record_job, set_commands, ReportTotals};
use op_slurm::slurm::{RequeuePolicy, SlurmJob, SlurmNode, SlurmNodes};

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

/// The width of every rule and table here, so the output fits the eighty
/// columns every terminal has.
const REPORT_WIDTH: usize = 80;

/// Projects listed in the cluster-wide table. Enough to see who is worst
/// affected without printing the whole machine.
const MAX_PROJECT_ROWS: usize = 20;

/// Nodes listed in the cluster-wide node-failure table.
const MAX_NODE_ROWS: usize = 15;

///
/// Days after which cluster-wide mode says it is going to be slow.
///
/// Not a refusal: the operator asked, and a long run that finishes is better
/// than a short one that answers the wrong question. But an unfiltered day of
/// a busy cluster is minutes of `sacct`, and knowing that before walking away
/// is worth a line of output.
///
const CLUSTER_WIDE_DAYS_BEFORE_WARNING: usize = 7;

///
/// How to rank the nodes Slurm blamed for losing work.
///
/// The two orderings answer different questions, and neither subsumes the
/// other. By hours lost, the top of the table is where the *work* went - and a
/// single failure that killed one very long job outranks a node that has failed
/// all week on short ones. By failure count, the same table finds the node that
/// keeps failing, which is the one to pull out of service whatever it happened
/// to be running at the time.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NodeOrder {
    /// Most hours lost first.
    #[default]
    ByUsage,
    /// Most failures first.
    ByFailures,
}

struct Options {
    /// The project to report on, or `None` for the whole cluster.
    project: Option<ProjectIdentifier>,
    dates: DateRange,
    node: String,
    sacct: String,
    cluster: String,
    requeue_policy: RequeuePolicy,
    node_order: NodeOrder,
}

const USAGE: &str = "\
Usage: get_requeue_report <project> <period> [options]
       get_requeue_report --cluster-wide <period> [options]

  <project>       the project, as OpenPortal spells it: project.portal
  <period>        today, yesterday, this_week, last_week, this_month,
                  last_month, this_year, last_year, a single date
                  (2026-08-01), or a range (2026-08-01:2026-08-14)

Options:
  --cluster-wide  report on every project on the cluster instead of one.
                  This reads every job on the machine for each day in the
                  period, so keep the period short - a day or two.
  --node JSON     the shape of a node on this cluster, as the slurm agent's
                  slurm-default-node option gives it. Defaults to the node
                  this tool was built for.
  --sacct CMD     the sacct command to run (default: sacct). Accepts a
                  composite command, e.g. 'docker exec slurmctld sacct'.
  --cluster NAME  restrict the query to one cluster
  --by-failures   rank the blamed-nodes table by how many times each node
                  failed, rather than by the hours it lost. Finds the node
                  that keeps failing, rather than the one that happened to
                  be running the longest job when it did. Cluster-wide only.
  --requeue-policy P
                  which requeued attempts count as usage, as the slurm
                  agent's requeue-policy option gives it:
                  charge_requeue_state_only (the default) or no_charge. Set
                  it to whatever the agent on this cluster is configured
                  with, or the charged/absorbed split will not match the
                  agent's.
  --help          show this message

Examples:
  get_requeue_report myproject.brics this_month
  get_requeue_report --cluster-wide yesterday
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
    let mut cluster_wide = false;
    let mut requeue_policy = RequeuePolicy::default();
    let mut node_order = NodeOrder::default();

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
            "--cluster-wide" => cluster_wide = true,
            "--by-failures" => node_order = NodeOrder::ByFailures,
            "--requeue-policy" => {
                let requested = value("--requeue-policy")?;
                requeue_policy = requested
                    .parse()
                    .map_err(|e| anyhow::anyhow!("{}. Try --help.", e))?;
            }
            other if other.starts_with('-') => {
                anyhow::bail!("Unknown option '{}'. Try --help.", other);
            }
            other => positional.push(other.to_string()),
        }
    }

    let (project, period) = match (cluster_wide, positional.as_slice()) {
        (true, [period]) => (None, period),
        (false, [project, period]) => {
            let project = ProjectIdentifier::parse(project).with_context(|| {
                format!(
                    "Could not read '{}' as a project. It is spelled project.portal.",
                    project
                )
            })?;

            (Some(project), period)
        }
        (true, _) => anyhow::bail!(
            "--cluster-wide takes a period and nothing else, got {} argument(s). Try --help.",
            positional.len()
        ),
        (false, _) => anyhow::bail!(
            "Expected a project and a period, got {} argument(s). Try --help.",
            positional.len()
        ),
    };

    if project.is_some() && node_order == NodeOrder::ByFailures {
        tracing::warn!(
            "--by-failures orders the blamed-nodes table, which only a --cluster-wide \
             report has. It will have no effect on a single project's report."
        );
    }

    let dates = DateRange::parse(period)
        .with_context(|| format!("Could not read '{}' as a period. Try --help.", period))?;

    Ok(Some(Options {
        project,
        dates,
        node,
        sacct,
        cluster,
        requeue_policy,
        node_order,
    }))
}

///
/// The Slurm account an OpenPortal project's jobs run under.
///
/// The account is named `{portal}.{project}`, which is the two halves of a
/// `ProjectIdentifier` the other way round.
///
fn account_of_project(project: &ProjectIdentifier) -> String {
    format!("{}.{}", project.portal(), project.project())
}

///
/// The OpenPortal project a Slurm account belongs to, if it belongs to one.
///
/// Anything that does not fit the `{portal}.{project}` shape is an account
/// OpenPortal did not create, and is none of this report's business: it is
/// counted as unmanaged rather than guessed at.
///
fn project_of_account(account: &str) -> Option<ProjectIdentifier> {
    let (portal, project) = account.trim().split_once('.')?;

    ProjectIdentifier::parse(&format!("{}.{}", project, portal)).ok()
}

/// What a node lost, when Slurm blamed it for losing work.
#[derive(Default, Clone, Copy)]
struct NodeFailures {
    events: u64,
    usage: u64,
}

/// Everything read, ready to be rendered.
#[derive(Default)]
struct Collected {
    /// project -> its whole report over the period
    projects: HashMap<ProjectIdentifier, ProjectUsageReport>,
    /// accounts seen that OpenPortal does not manage, and their requeue events
    unmanaged: HashMap<String, u64>,
    /// nodes Slurm blamed for a `NODE_FAIL`, and what they cost
    failed_nodes: HashMap<String, NodeFailures>,
    /// distinct jobs with at least one superseded attempt. An event count says
    /// how often requeueing happened; this says how many jobs it happened to,
    /// which is the figure a project recognises.
    requeued_jobs: std::collections::HashSet<u64>,
    /// jobs that had not finished, so their runtimes are not in the report
    saw_unfinished_job: bool,
    /// the days actually read, in order
    days: Vec<Date>,
    /// days that could not be read whole, and how many hours are missing
    gaps: Vec<(Date, usize)>,
}

///
/// Read the whole period, one day at a time.
///
async fn collect(options: &Options, nodes: &SlurmNodes) -> Result<Collected> {
    let now = chrono::Utc::now();
    let mut collected = Collected::default();

    // A day that has not started cannot have been used, and `sacct` has nothing
    // to say about it.
    let days: Vec<Date> = options
        .dates
        .days()
        .into_iter()
        .filter(|day| day.day().start_time().and_utc() <= now)
        .collect();

    let total_days = days.len();

    if total_days == 0 {
        tracing::warn!("The period '{}' has not started yet", options.dates);
    }

    let query = JobQuery {
        cluster: options.cluster.clone(),
        // One project's report asks `sacct` for one account, which is worth a
        // great deal on a busy cluster. Cluster-wide has to read everything -
        // the question is who this happened to, and that is not known until
        // the records have been read.
        account: match &options.project {
            Some(project) => account_of_project(project),
            None => String::new(),
        },
        reservation: String::new(),
    };

    match &options.project {
        Some(project) => tracing::info!(
            "Reading requeues for project {} over {} day(s)",
            project,
            total_days
        ),
        None => {
            tracing::info!(
                "Reading requeues for every project over {} day(s). This reads every job \
                 on the cluster.",
                total_days
            );

            if total_days > CLUSTER_WIDE_DAYS_BEFORE_WARNING {
                tracing::warn!(
                    "{} days is a lot to ask for cluster-wide - every day is an unfiltered \
                     query over the whole machine. Expect this to take a while, and consider \
                     a shorter period.",
                    total_days
                );
            }
        }
    }

    for (index, day) in days.iter().enumerate() {
        tracing::info!("Processing day {} of {} ({})", index + 1, total_days, day);

        let DayRecords {
            jobs,
            missing_hours,
        } = jobs_on_day(day, nodes, &query, &now).await;

        if missing_hours > 0 {
            collected.gaps.push((day.clone(), missing_hours));
        }

        let requeued = jobs.iter().filter(|job| job.is_requeued_attempt()).count();

        absorb_day(&mut collected, &jobs, day, options.requeue_policy);

        tracing::info!(
            "{}: {} record(s) read, {} of them superseded by a requeue",
            day,
            jobs.len(),
            requeued
        );
    }

    if !collected.gaps.is_empty() {
        let missing: usize = collected
            .gaps
            .iter()
            .map(|(_, hours)| *hours)
            .fold(0usize, |total, hours| total.saturating_add(hours));

        tracing::warn!(
            "{} hour(s) across {} day(s) could not be read from Slurm. Every figure in \
             this report is therefore a lower bound.",
            missing,
            collected.gaps.len()
        );
    }

    Ok(collected)
}

///
/// Fold one day's records into the report.
///
/// One `DailyProjectUsageReport` per project per day, exactly as the agent
/// builds them, so that `record_job` counts a job in the window it started in
/// and the charged/absorbed split is the agent's own.
///
fn absorb_day(collected: &mut Collected, jobs: &[SlurmJob], day: &Date, policy: RequeuePolicy) {
    collected.days.push(day.clone());

    let start_time = day.day().start_time().and_utc();

    let mut days: HashMap<ProjectIdentifier, (DailyProjectUsageReport, ReportTotals)> =
        HashMap::new();

    for job in jobs {
        // A node failure is a fact about the hardware rather than about
        // billing, so it is counted whoever ran the job - including on an
        // account OpenPortal does not manage. The caption on that table says
        // so, because it is the one figure here that is not about projects.
        if job.is_requeued_attempt()
            && job.terminal_state() == "NODE_FAIL"
            && !job.failed_node().is_empty()
        {
            let node = collected
                .failed_nodes
                .entry(job.failed_node().to_string())
                .or_default();

            node.events = node.events.saturating_add(1);
            node.usage = node.usage.saturating_add(job.billed_node_seconds());
        }

        let Some(project) = project_of_account(job.account()) else {
            if job.is_requeued_attempt() {
                *collected
                    .unmanaged
                    .entry(job.account().to_string())
                    .or_default() += 1;
            }
            continue;
        };

        // Counted only for managed projects, because it is printed beside the
        // project counts and would otherwise be a larger number drawn from a
        // different population.
        if job.is_requeued_attempt() {
            collected.requeued_jobs.insert(job.id());
        }

        let (report, totals) = days.entry(project).or_default();
        record_job(report, job, &start_time, policy, totals);
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

/// Everything requeueing cost a project - charged and absorbed together.
fn requeued_total(report: &ProjectUsageReport) -> Usage {
    report.total_requeue_usage_including_charged()
}

/// The share of a project's true consumption that went on requeued attempts,
/// as a percentage. This is the figure that says a project is in trouble.
fn requeue_share(report: &ProjectUsageReport) -> f64 {
    match report.total_usage_including_requeues().seconds() {
        0 => 0.0,
        total => 100.0 * requeued_total(report).seconds() as f64 / total as f64,
    }
}

/// Projects worst-affected first, by what requeueing cost them.
fn projects_by_requeue(collected: &Collected) -> Vec<(&ProjectIdentifier, &ProjectUsageReport)> {
    let mut projects: Vec<(&ProjectIdentifier, &ProjectUsageReport)> = collected
        .projects
        .iter()
        .filter(|(_, report)| !requeued_total(report).is_zero())
        .collect();

    projects.sort_by(|a, b| {
        requeued_total(b.1)
            .seconds()
            .cmp(&requeued_total(a.1).seconds())
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });

    projects
}

/// Sum a figure over every project.
fn cluster_total(collected: &Collected, of: impl Fn(&ProjectUsageReport) -> Usage) -> Usage {
    collected.projects.values().map(of).sum()
}

/// Sum a count over every project.
fn cluster_count(collected: &Collected, of: impl Fn(&ProjectUsageReport) -> u64) -> u64 {
    collected
        .projects
        .values()
        .fold(0u64, |total, report| total.saturating_add(of(report)))
}

///
/// What to say about the parts of the period that could not be read.
///
/// Slurm refusing an hour is not the same as nothing having been requeued in
/// it, and a report that quietly conflated the two would understate exactly the
/// thing it exists to measure.
///
fn gaps_note(collected: &Collected) -> String {
    let mut out = String::new();

    let Some((first, rest)) = collected.gaps.split_first() else {
        return out;
    };

    let missing = collected
        .gaps
        .iter()
        .map(|(_, hours)| *hours)
        .fold(0usize, |total, hours| total.saturating_add(hours));

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "INCOMPLETE: {} hour(s) could not be read from Slurm, so every figure below is",
        missing
    );
    let _ = writeln!(
        out,
        "a lower bound - what ran in those hours is missing from it entirely."
    );

    let _ = write!(out, "Affected: {} ({}h)", first.0, first.1);

    for (day, hours) in rest {
        let _ = write!(out, ", {} ({}h)", day, hours);
    }

    let _ = writeln!(out);

    out
}

///
/// One project's report.
///
/// The body is `ProjectUsageReport::requeue_report()` - the agent's own
/// rendering, so that what an operator reads here and what a support ticket
/// quotes are the same text.
///
fn render_project(project: &ProjectIdentifier, collected: &Collected) -> String {
    let mut out = String::new();

    let Some(report) = collected.projects.get(project) else {
        let _ = writeln!(out, "Requeue summary for {}", project);
        let _ = writeln!(out, "{}", "=".repeat(64));
        let _ = writeln!(
            out,
            "No jobs at all were recorded for this project over this period."
        );
        let _ = writeln!(out, "{}", "=".repeat(64));
        return out;
    };

    let _ = write!(out, "{}", gaps_note(collected));
    let _ = write!(out, "{}", report.requeue_report());

    if collected.saw_unfinished_job {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Some jobs had not finished when this was read. Their usage so far is"
        );
        let _ = writeln!(out, "counted; their runtimes are not.");
    }

    out
}

///
/// The cluster-wide summary.
///
fn render_cluster(collected: &Collected, node_order: NodeOrder) -> String {
    let mut out = String::new();
    let rule = "=".repeat(REPORT_WIDTH);

    let first = collected.days.first();
    let last = collected.days.last();

    let _ = writeln!(out, "{}", rule);
    match (first, last) {
        (Some(first), Some(last)) if first == last => {
            let _ = writeln!(out, "Cluster-wide requeue summary for {}", first);
        }
        (Some(first), Some(last)) => {
            let _ = writeln!(out, "Cluster-wide requeue summary, {} to {}", first, last);
        }
        _ => {
            let _ = writeln!(out, "Cluster-wide requeue summary");
        }
    }
    let _ = writeln!(out, "{}", rule);

    let _ = write!(out, "{}", gaps_note(collected));

    let reported = cluster_total(collected, |r| r.total_usage());
    let absorbed = cluster_total(collected, |r| r.total_requeue_usage());
    let charged = cluster_total(collected, |r| r.total_charged_requeue_usage());
    let truth = cluster_total(collected, |r| r.total_usage_including_requeues());
    let requeued = Usage::new(absorbed.seconds().saturating_add(charged.seconds()));

    let percent = |part: &Usage| match truth.seconds() {
        0 => 0.0,
        total => 100.0 * part.seconds() as f64 / total as f64,
    };

    if truth.is_zero() {
        let _ = writeln!(out);
        let _ = writeln!(out, "No usage at all was recorded over this period.");
        let _ = writeln!(out, "{}", rule);
        return out;
    }

    let events = cluster_count(collected, |r| r.num_requeue_events())
        .saturating_add(cluster_count(collected, |r| r.num_charged_requeue_events()));
    let jobs = cluster_count(collected, |r| r.num_jobs());

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Projects with jobs               : {:>14}",
        collected.projects.len()
    );
    let _ = writeln!(out, "Jobs                             : {:>14}", jobs);
    let _ = writeln!(
        out,
        "True consumption (Slurm's view)  : {:>14}",
        truth.in_hours().to_string()
    );
    let _ = writeln!(
        out,
        "Lost to requeues                 : {:>14}  ({:.1}%)",
        requeued.in_hours().to_string(),
        percent(&requeued)
    );
    let _ = writeln!(
        out,
        "  charged to the projects        : {:>14}  ({:.1}%)",
        charged.in_hours().to_string(),
        percent(&charged)
    );
    let _ = writeln!(
        out,
        "  absorbed by the site           : {:>14}  ({:.1}%)",
        absorbed.in_hours().to_string(),
        percent(&absorbed)
    );
    let _ = writeln!(
        out,
        "Usage reported to the portal     : {:>14}",
        reported.in_hours().to_string()
    );

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} requeue {} across {} distinct {}, on {} of {} {} with any",
        events,
        match events == 1 {
            true => "event",
            false => "events",
        },
        collected.requeued_jobs.len(),
        match collected.requeued_jobs.len() == 1 {
            true => "job",
            false => "jobs",
        },
        projects_by_requeue(collected).len(),
        collected.projects.len(),
        match collected.projects.len() == 1 {
            true => "project",
            false => "projects",
        }
    );

    let wait =
        cluster_count(collected, |r| r.requeue_wait_seconds())
            .saturating_add(cluster_count(collected, |r| {
                r.charged_requeue_wait_seconds()
            }));

    let _ = writeln!(
        out,
        "Queue wait thrown away: {} in total, {} per requeue",
        Usage::new(wait).in_hours(),
        Usage::new(match events {
            0 => 0,
            events => wait / events,
        })
        .in_hours()
    );

    let _ = writeln!(
        out,
        "Mean usage lost per requeue: {}",
        Usage::new(match events {
            0 => 0,
            events => requeued.seconds() / events,
        })
        .in_hours()
    );

    let _ = write!(out, "{}", render_states(collected, &percent));
    let _ = write!(out, "{}", render_projects(collected));
    let _ = write!(out, "{}", render_days(collected));
    let _ = write!(out, "{}", render_failed_nodes(collected, node_order));
    let _ = write!(out, "{}", render_unmanaged(collected));

    let _ = writeln!(out, "{}", rule);

    out
}

/// What did the interrupting, cluster-wide - the figure that separates the
/// site's own faults from the users' choices.
fn render_states(collected: &Collected, percent: &impl Fn(&Usage) -> f64) -> String {
    let mut out = String::new();

    let mut events: HashMap<String, u64> = HashMap::new();
    let mut usage: HashMap<String, Usage> = HashMap::new();
    let mut charged: HashMap<String, bool> = HashMap::new();

    for report in collected.projects.values() {
        for (state, state_events, state_usage) in report.requeue_state_summary() {
            *events.entry(state.clone()).or_default() += state_events;
            *usage.entry(state.clone()).or_default() += state_usage;
            charged.entry(state).or_insert(false);
        }

        for (state, state_events, state_usage) in report.charged_requeue_state_summary() {
            *events.entry(state.clone()).or_default() += state_events;
            *usage.entry(state.clone()).or_default() += state_usage;
            charged.insert(state, true);
        }
    }

    if events.is_empty() && usage.is_empty() {
        return out;
    }

    let mut states: Vec<String> = events.keys().chain(usage.keys()).cloned().collect();
    states.sort();
    states.dedup();
    states.sort_by(|a, b| {
        usage
            .get(b)
            .cloned()
            .unwrap_or_default()
            .seconds()
            .cmp(&usage.get(a).cloned().unwrap_or_default().seconds())
            .then_with(|| a.cmp(b))
    });

    let _ = writeln!(out);
    let _ = writeln!(out, "Work was interrupted by:");

    for state in states {
        let state_events = events.get(&state).copied().unwrap_or(0);
        let state_usage = usage.get(&state).cloned().unwrap_or_default();

        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>12}  ({:>5.1}%)  {}",
            state,
            state_events,
            match state_events == 1 {
                true => "event",
                false => "events",
            },
            state_usage.in_hours().to_string(),
            percent(&state_usage),
            match charged.get(&state).copied().unwrap_or(false) {
                true => "charged",
                false => "absorbed",
            }
        );
    }

    out
}

/// The projects worst affected, which is the question cluster-wide mode exists
/// to answer.
fn render_projects(collected: &Collected) -> String {
    let mut out = String::new();

    let projects = projects_by_requeue(collected);

    if projects.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "No project lost any work to a requeue.");
        return out;
    }

    let (rows, rest) = projects.split_at(projects.len().min(MAX_PROJECT_ROWS));

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Worst affected projects, by what requeueing cost them:"
    );
    let _ = writeln!(out, "{}", "-".repeat(REPORT_WIDTH));
    let _ = writeln!(
        out,
        "{:<24} {:>10} {:>10} {:>10} {:>6} {:>6}",
        "project", "lost", "charged", "absorbed", "share", "events"
    );

    for (project, report) in rows {
        let events = report
            .num_requeue_events()
            .saturating_add(report.num_charged_requeue_events());

        let _ = writeln!(
            out,
            "{:<24} {:>10.2} {:>10.2} {:>10.2} {:>5.1}% {:>6}",
            elide(&project.to_string(), 24),
            requeued_total(report).hours(),
            report.total_charged_requeue_usage().hours(),
            report.total_requeue_usage().hours(),
            requeue_share(report),
            events
        );
    }

    if !rest.is_empty() {
        let rest_usage: Usage = rest.iter().map(|(_, report)| requeued_total(report)).sum();

        let _ = writeln!(
            out,
            "{:<24} {:>10.2}  ... and {} more project(s)",
            "(the rest)",
            rest_usage.hours(),
            rest.len()
        );
    }

    let _ = writeln!(
        out,
        "'share' is what requeueing cost the project as a percentage of everything it"
    );
    let _ = writeln!(out, "consumed, so a small project can top it on a bad day.");

    out
}

/// Day by day, so that an incident shows up as an incident rather than as a
/// slightly larger total.
fn render_days(collected: &Collected) -> String {
    let mut out = String::new();

    if collected.days.len() < 2 {
        return out;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "Day by day:");
    let _ = writeln!(out, "{}", "-".repeat(REPORT_WIDTH));
    let _ = writeln!(
        out,
        "{:<12} {:>10} {:>10} {:>8} {:>10}",
        "date", "lost", "of which", "events", "projects"
    );
    let _ = writeln!(
        out,
        "{:<12} {:>10} {:>10} {:>8} {:>10}",
        "", "(hours)", "charged", "", "affected"
    );

    for day in &collected.days {
        let mut lost = Usage::default();
        let mut charged = Usage::default();
        let mut events = 0u64;
        let mut projects = 0usize;

        for report in collected.projects.values() {
            let daily = report.get_report(day);

            let day_lost = daily.total_requeue_usage_including_charged();

            if day_lost.is_zero() && daily.num_requeue_events() == 0 {
                continue;
            }

            projects += 1;
            lost += day_lost;
            charged += daily.total_charged_requeue_usage();
            events = events.saturating_add(
                daily
                    .num_requeue_events()
                    .saturating_add(daily.num_charged_requeue_events()),
            );
        }

        let _ = writeln!(
            out,
            "{:<12} {:>10.2} {:>10.2} {:>8} {:>10}",
            day.to_string(),
            lost.hours(),
            charged.hours(),
            events,
            projects
        );
    }

    out
}

///
/// The nodes Slurm blamed for losing work.
///
/// This is the actionable half of the report: `NODE_FAIL` time is the site's
/// own, and a node that appears here repeatedly is a node to look at.
///
fn render_failed_nodes(collected: &Collected, order: NodeOrder) -> String {
    let mut out = String::new();

    if collected.failed_nodes.is_empty() {
        return out;
    }

    let mut nodes: Vec<(&String, &NodeFailures)> = collected.failed_nodes.iter().collect();

    // Whichever key is not the primary one still breaks the ties, so the order
    // is total either way and the table does not shuffle between runs.
    nodes.sort_by(|a, b| match order {
        NodeOrder::ByUsage => {
            b.1.usage
                .cmp(&a.1.usage)
                .then_with(|| b.1.events.cmp(&a.1.events))
                .then_with(|| a.0.cmp(b.0))
        }
        NodeOrder::ByFailures => {
            b.1.events
                .cmp(&a.1.events)
                .then_with(|| b.1.usage.cmp(&a.1.usage))
                .then_with(|| a.0.cmp(b.0))
        }
    });

    let (rows, rest) = nodes.split_at(nodes.len().min(MAX_NODE_ROWS));

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Nodes Slurm blamed for losing work, {}:",
        match order {
            NodeOrder::ByUsage => "by hours lost",
            NodeOrder::ByFailures => "by how often each failed",
        }
    );
    let _ = writeln!(out, "{}", "-".repeat(REPORT_WIDTH));
    let _ = writeln!(out, "{:<24} {:>10} {:>8}", "node", "lost", "failures");

    for (node, failures) in rows {
        let _ = writeln!(
            out,
            "{:<24} {:>10.2} {:>8}",
            elide(node, 24),
            Usage::new(failures.usage).hours(),
            failures.events
        );
    }

    if !rest.is_empty() {
        let _ = writeln!(out, "... and {} more node(s)", rest.len());
    }

    let _ = writeln!(
        out,
        "Only failures Slurm named a node for are here; it does not always name one."
    );

    if order == NodeOrder::ByUsage && nodes.len() > MAX_NODE_ROWS {
        let _ = writeln!(
            out,
            "A node that fails often on short jobs will not be near the top of this"
        );
        let _ = writeln!(out, "ordering - --by-failures finds those.");
    }

    out
}

/// Accounts that are not OpenPortal projects, so the totals above are honest
/// about what they leave out.
fn render_unmanaged(collected: &Collected) -> String {
    let mut out = String::new();

    if collected.unmanaged.is_empty() {
        return out;
    }

    let mut accounts: Vec<(&String, &u64)> = collected.unmanaged.iter().collect();
    accounts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    let events: u64 = accounts
        .iter()
        .fold(0u64, |total, (_, count)| total.saturating_add(**count));

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} requeue event(s) were on {} account(s) OpenPortal does not manage, and are",
        events,
        accounts.len()
    );
    let _ = writeln!(out, "not in any figure above:");

    for (account, count) in accounts.iter().take(MAX_PROJECT_ROWS) {
        let _ = writeln!(out, "  {:<24} {:>5}", elide(account, 24), count);
    }

    out
}

/// Trim a name that would break its column, marking that it was trimmed.
fn elide(name: &str, width: usize) -> String {
    match name.chars().count() > width {
        false => name.to_string(),
        true => match width > 1 {
            true => format!("{}…", name.chars().take(width - 1).collect::<String>()),
            false => name.chars().take(width).collect::<String>(),
        },
    }
}

fn render(options: &Options, collected: &Collected) -> String {
    match &options.project {
        Some(project) => render_project(project, collected),
        None => render_cluster(collected, options.node_order),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // progress goes to standard error, the report to standard output, so that
    // `get_requeue_report ... > report.txt` leaves a clean file
    templemeads::config::initialise_tracing_to_stderr();

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The day the fixture covers: 2026-03-01.
    const DAY_ONE: i64 = 1772323200;

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

    ///
    /// The fixture's records, classified as `sacct` would have them returned.
    ///
    /// Through `get_consumers` rather than hand-built, because whether an
    /// attempt is recognised as superseded depends entirely on which of a job's
    /// other attempts the query returned - which is the thing this tool is
    /// reporting on.
    ///
    fn fixture_jobs() -> Vec<SlurmJob> {
        let Ok(response) = serde_json::from_str::<serde_json::Value>(include_str!(
            "../tests/data/sacct-cluster-requeues.json"
        )) else {
            unreachable!("the fixture is JSON");
        };

        let start = day(DAY_ONE).day().start_time().and_utc();
        let end = day(DAY_ONE).day().end_time().and_utc();

        let Ok(jobs) = SlurmJob::get_consumers(&response, &start, &end, &test_nodes()) else {
            unreachable!("the fixture parses");
        };

        jobs
    }

    fn collected_fixture(policy: RequeuePolicy) -> Collected {
        let mut collected = Collected::default();

        absorb_day(&mut collected, &fixture_jobs(), &day(DAY_ONE), policy);

        collected
    }

    fn project(name: &str) -> ProjectIdentifier {
        let Ok(project) = ProjectIdentifier::parse(name) else {
            unreachable!("this is a well-formed project identifier");
        };

        project
    }

    #[test]
    fn test_a_slurm_account_and_an_openportal_project_name_each_other() {
        // The two halves are spelled the other way round, and getting it wrong
        // would query an account that does not exist and report nothing at all.
        let project = project("aaaa.brics");

        assert_eq!(account_of_project(&project), "brics.aaaa");
        assert_eq!(project_of_account("brics.aaaa"), Some(project));

        // and an account OpenPortal did not create is not guessed at
        assert_eq!(project_of_account("root"), None);
    }

    #[test]
    fn test_every_project_in_the_fixture_is_found_and_the_rest_declared() {
        let collected = collected_fixture(RequeuePolicy::default());

        assert_eq!(collected.projects.len(), 3);
        assert!(collected.projects.contains_key(&project("aaaa.brics")));
        assert!(collected.projects.contains_key(&project("bbbb.brics")));
        assert!(collected.projects.contains_key(&project("cccc.brics")));

        // the unmanaged account is counted rather than silently dropped, and
        // is not one of the projects
        assert_eq!(collected.unmanaged.get("root"), Some(&1));
        assert!(!collected.projects.contains_key(&project("root.root")));
    }

    #[test]
    fn test_the_charged_and_absorbed_split_is_the_agents_own() {
        let collected = collected_fixture(RequeuePolicy::ChargeRequeueStateOnly);

        let Some(aaaa) = collected.projects.get(&project("aaaa.brics")) else {
            unreachable!("the fixture has this project");
        };

        // bob's job was requeued at his own asking - three hours, charged
        assert_eq!(aaaa.total_charged_requeue_usage(), Usage::new(10800));

        // alice's was lost to a node failure - two hours, absorbed
        assert_eq!(aaaa.total_requeue_usage(), Usage::new(7200));

        // and under no_charge the same five hours are all absorbed
        let collected = collected_fixture(RequeuePolicy::NoCharge);

        let Some(aaaa) = collected.projects.get(&project("aaaa.brics")) else {
            unreachable!("the fixture has this project");
        };

        assert!(aaaa.total_charged_requeue_usage().is_zero());
        assert_eq!(aaaa.total_requeue_usage(), Usage::new(10800 + 7200));
    }

    #[test]
    fn test_the_cluster_report_ranks_projects_by_what_requeueing_cost_them() {
        let collected = collected_fixture(RequeuePolicy::default());
        let report = render_cluster(&collected, NodeOrder::default());

        let position = |needle: &str| report.find(needle);

        // aaaa lost five hours, bbbb one, cccc half of one
        let (Some(aaaa), Some(bbbb), Some(cccc)) = (
            position("aaaa.brics"),
            position("bbbb.brics"),
            position("cccc.brics"),
        ) else {
            unreachable!("every project appears in the report: {}", report);
        };

        assert!(aaaa < bbbb, "{}", report);
        assert!(bbbb < cccc, "{}", report);
    }

    #[test]
    fn test_the_cluster_report_separates_the_sites_faults_from_the_users_choices() {
        let collected = collected_fixture(RequeuePolicy::default());
        let report = render_cluster(&collected, NodeOrder::default());

        // three hours of node failure and half an hour of preemption are the
        // site's; three hours of plain requeue are the user's
        assert!(report.contains("NODE_FAIL"), "{}", report);
        assert!(report.contains("PREEMPTED"), "{}", report);
        assert!(report.contains("REQUEUED"), "{}", report);

        assert!(report.contains("absorbed by the site"), "{}", report);
        assert!(report.contains("charged to the projects"), "{}", report);

        // every state line says which way it went
        let states: Vec<&str> = report
            .lines()
            .skip_while(|line| !line.starts_with("Work was interrupted by:"))
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .collect();

        assert_eq!(states.len(), 3, "{}", report);

        for line in states {
            assert!(
                line.ends_with("charged") || line.ends_with("absorbed"),
                "{}",
                line
            );
        }
    }

    #[test]
    fn test_the_cluster_report_names_the_node_that_lost_the_work() {
        // The actionable half: a node that appears here repeatedly is a node to
        // look at. The fixture has one node losing two jobs, in two different
        // projects, which is exactly the pattern a per-project report cannot
        // show.
        let collected = collected_fixture(RequeuePolicy::default());

        let Some(failures) = collected.failed_nodes.get("badnode01") else {
            unreachable!("the fixture blames this node");
        };

        assert_eq!(failures.events, 2);
        assert_eq!(failures.usage, 7200 + 3600);

        let report = render_cluster(&collected, NodeOrder::default());
        assert!(report.contains("badnode01"), "{}", report);
        assert!(report.contains("Nodes Slurm blamed"), "{}", report);
    }

    /// Nodes in the shape production actually has them: one that failed once
    /// while running something enormous, and one that keeps failing on small
    /// work. Which of the two is "worst" is the question the flag answers.
    fn collected_with_nodes() -> Collected {
        let mut collected = Collected::default();

        collected.failed_nodes.insert(
            "unlucky01".to_string(),
            NodeFailures {
                events: 1,
                usage: 3600 * 600,
            },
        );
        collected.failed_nodes.insert(
            "sick01".to_string(),
            NodeFailures {
                events: 8,
                usage: 3600 * 200,
            },
        );
        collected.failed_nodes.insert(
            "quiet01".to_string(),
            NodeFailures {
                events: 1,
                usage: 3600 * 50,
            },
        );

        collected
    }

    #[test]
    fn test_the_node_table_can_be_ranked_by_hours_or_by_failures() {
        let collected = collected_with_nodes();

        let by_usage = render_failed_nodes(&collected, NodeOrder::ByUsage);
        let by_failures = render_failed_nodes(&collected, NodeOrder::ByFailures);

        let first_node = |report: &str| -> String {
            report
                .lines()
                .skip_while(|line| !line.starts_with("node "))
                .nth(1)
                .unwrap_or_default()
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        };

        // the node that lost the most work, against the node that keeps failing
        assert_eq!(first_node(&by_usage), "unlucky01", "{}", by_usage);
        assert_eq!(first_node(&by_failures), "sick01", "{}", by_failures);

        // and the heading says which question is being answered, so a report
        // pasted into a ticket cannot be read as the other one
        assert!(by_usage.contains("by hours lost"), "{}", by_usage);
        assert!(
            by_failures.contains("by how often each failed"),
            "{}",
            by_failures
        );
    }

    #[test]
    fn test_the_node_ordering_is_total_so_the_table_does_not_shuffle() {
        // Both keys tie on `quiet01` and `unlucky01` in one ordering or the
        // other, so without a name to break the tie the table would come out
        // differently on each run of the same data - which is exactly what
        // makes two reports impossible to compare.
        let collected = collected_with_nodes();

        for order in [NodeOrder::ByUsage, NodeOrder::ByFailures] {
            let first = render_failed_nodes(&collected, order);

            for _ in 0..8 {
                assert_eq!(render_failed_nodes(&collected, order), first);
            }
        }
    }

    #[test]
    fn test_a_cluster_with_no_blamed_nodes_prints_no_table() {
        // Slurm does not always name a node, and a heading over nothing reads
        // as though it did and found none.
        let collected = Collected::default();

        assert!(render_failed_nodes(&collected, NodeOrder::ByUsage).is_empty());
    }

    #[test]
    fn test_distinct_jobs_are_counted_apart_from_events() {
        // A job requeued four times is four events and one job, and the two
        // answer different questions. Only managed projects are counted here,
        // because the figure is printed beside the project counts.
        let collected = collected_fixture(RequeuePolicy::default());

        assert_eq!(collected.requeued_jobs.len(), 4);
        assert!(!collected.requeued_jobs.contains(&600));

        let report = render_cluster(&collected, NodeOrder::default());
        assert!(report.contains("distinct job"), "{}", report);
    }

    #[test]
    fn test_the_cluster_report_declares_the_accounts_it_left_out() {
        let collected = collected_fixture(RequeuePolicy::default());
        let report = render_cluster(&collected, NodeOrder::default());

        assert!(report.contains("does not manage"), "{}", report);
        assert!(report.contains("root"), "{}", report);
    }

    #[test]
    fn test_a_period_with_a_gap_in_it_says_so_before_any_figures() {
        // An hour Slurm would not answer for is not an hour in which nothing
        // was requeued, and this report must not let the two look alike.
        let mut collected = collected_fixture(RequeuePolicy::default());
        collected.gaps.push((day(DAY_ONE), 2));

        let report = render_cluster(&collected, NodeOrder::default());

        assert!(report.contains("INCOMPLETE"), "{}", report);
        assert!(report.contains("2026-03-01 (2h)"), "{}", report);

        let (Some(warning), Some(table)) = (
            report.find("INCOMPLETE"),
            report.find("Worst affected projects"),
        ) else {
            unreachable!("both appear in the report: {}", report);
        };

        assert!(warning < table);
    }

    #[test]
    fn test_a_single_project_report_is_the_agents_own_rendering() {
        // Quoting the agent's own text means a support ticket and the agent's
        // logs cannot disagree about what a project was told.
        let collected = collected_fixture(RequeuePolicy::default());
        let report = render_project(&project("aaaa.brics"), &collected);

        assert!(
            report.contains("Requeue summary for aaaa.brics"),
            "{}",
            report
        );
        assert!(report.contains("True consumption"), "{}", report);
        assert!(report.contains("NODE_FAIL"), "{}", report);

        // and another project's requeues are not in it
        assert!(!report.contains("cccc.brics"), "{}", report);
    }

    #[test]
    fn test_a_project_with_no_jobs_says_so_rather_than_printing_nothing() {
        let collected = collected_fixture(RequeuePolicy::default());
        let report = render_project(&project("nobody.brics"), &collected);

        assert!(report.contains("No jobs at all"), "{}", report);
    }

    #[test]
    fn test_a_cluster_with_nothing_on_it_says_so_rather_than_dividing_by_zero() {
        let collected = Collected::default();
        let report = render_cluster(&collected, NodeOrder::default());

        assert!(report.contains("No usage at all"), "{}", report);
    }

    #[test]
    fn test_a_long_name_is_trimmed_rather_than_breaking_the_column() {
        assert_eq!(elide("short", 24), "short");
        assert_eq!(elide(&"x".repeat(30), 5), "xxxx…");
    }
}
