// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! Reading a day of Slurm accounting records, for the operator tools.
//!
//! The agent asks Slurm for one project's day at a time and falls back to
//! hourly queries when that fails. The tools in `tools/` have the same problem
//! and a worse version of it - a tool asking for *every* account's day is the
//! heaviest query anything here makes - so the narrowing lives here rather than
//! in each tool, and there is one copy of the rules about what to do when
//! `sacct` will not answer.
//!
//! This is deliberately not the agent's own path: the agent queries one account,
//! has a job's deadline to respect and a cache to fill, and none of that applies
//! to a person waiting at a terminal.
//!

use anyhow::Result;

use greatwestern::grammar::Date;

use crate::sacctmgr::runner;
use crate::slurm::{SlurmJob, SlurmNodes};

///
/// How long to wait for one query.
///
/// Generous compared with the agent's own thirty seconds. A tool run by hand
/// can afford to wait; being told "timed out" is not an answer an operator can
/// use.
///
pub const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

///
/// Which records to ask `sacct` for, beyond the window itself.
///
/// Every field narrows the query. An empty one asks for everything, which is
/// what a tool wants when the question is "who did this happen to" rather than
/// "what did this project do".
///
#[derive(Debug, Clone, Default)]
pub struct JobQuery {
    /// Restrict to one cluster, empty for the default.
    pub cluster: String,
    /// Restrict to one Slurm account, empty for every account. Worth a great
    /// deal on a busy cluster - the alternative is reading every job on the
    /// machine and discarding nearly all of them.
    pub account: String,
    /// Restrict to one reservation, empty for every job.
    ///
    /// `--reservation` is not available on every `sacct` this may meet, and one
    /// that quietly means something else would silently change what a report
    /// covers, so a caller that sets this should filter the records again
    /// itself and be able to run without it.
    pub reservation: String,
}

impl JobQuery {
    fn args(&self) -> Vec<String> {
        let mut args = Vec::new();

        if !self.cluster.is_empty() {
            args.push(format!("--cluster={}", self.cluster));
        }

        if !self.account.is_empty() {
            args.push(format!("--account={}", self.account));
        }

        if !self.reservation.is_empty() {
            args.push(format!("--reservation={}", self.reservation));
        }

        args
    }
}

/// What one day's reading came to: the records, and how many of the day's
/// hours could not be read at all.
pub struct DayRecords {
    pub jobs: Vec<SlurmJob>,
    pub missing_hours: usize,
}

///
/// Ask Slurm for every job that overlapped one window.
///
pub async fn jobs_between(
    start_time: &chrono::DateTime<chrono::Utc>,
    end_time: &chrono::DateTime<chrono::Utc>,
    nodes: &SlurmNodes,
    query: &JobQuery,
    now: &chrono::DateTime<chrono::Utc>,
    timeout: std::time::Duration,
) -> Result<Vec<SlurmJob>> {
    // a long expiry: this is a one-shot tool with a person waiting on it, not
    // an agent servicing a job with a deadline
    let expires = *now + chrono::Duration::hours(1);

    let mut args = vec![
        "--noconvert".to_string(),
        "--allocations".to_string(),
        "--allusers".to_string(),
        // one record per attempt - without this everything a requeued job
        // consumed before its final attempt is invisible
        "--duplicates".to_string(),
        format!("--starttime={}", start_time.format("%Y-%m-%dT%H:%M:%S")),
        format!("--endtime={}", end_time.format("%Y-%m-%dT%H:%M:%S")),
    ];

    args.extend(query.args());
    args.push("--json".to_string());

    let cmd = runner(&expires).await?.build_command("SACCT", args)?;

    let response = runner(&expires).await?.run_json(&cmd, timeout).await?;

    Ok(SlurmJob::get_consumers(
        &response, start_time, end_time, nodes,
    )?)
}

///
/// Ask Slurm for every job that ran on one day.
///
/// A day is asked for in one query first, because that is one query rather
/// than twenty-four. When that query fails - however it fails - the day is
/// asked for an hour at a time instead. An unfiltered day on a busy cluster is
/// the heaviest thing these tools ask of `sacct`, and it does not always come
/// back: it can be killed for running out of memory, cut off by a limit on the
/// scheduler's side, or time out outright. Those exit in different ways but
/// they mean the same thing - the window was too wide - and the answer to all
/// of them is a narrower window.
///
/// An hour that fails in turn is skipped rather than fatal, and counted. There
/// is nothing smaller left to try, and a report that is honest about the hour
/// it is missing is worth far more to an operator than no report at all - not
/// least because this tends to happen near the end of a long run, with every
/// day before it already read.
///
pub async fn jobs_on_day(
    day: &Date,
    nodes: &SlurmNodes,
    query: &JobQuery,
    now: &chrono::DateTime<chrono::Utc>,
) -> DayRecords {
    let start_time = day.day().start_time().and_utc();
    let end_time = day.day().end_time().and_utc();

    if start_time > *now {
        return DayRecords {
            jobs: Vec::new(),
            missing_hours: 0,
        };
    }

    // never ask for the future - `sacct` is happy to be asked and the clipping
    // would treat the rest of today as consumed
    let end_time = end_time.min(*now);

    let day_error =
        match jobs_between(&start_time, &end_time, nodes, query, now, QUERY_TIMEOUT).await {
            Ok(jobs) => {
                return DayRecords {
                    jobs,
                    missing_hours: 0,
                }
            }
            Err(e) => e,
        };

    tracing::warn!(
        "Could not read {} in one query: {:#}. Reading it an hour at a time instead.",
        day,
        day_error
    );

    let mut jobs = Vec::new();
    let mut missing_hours: usize = 0;

    for hour in day.hours() {
        let hour_start = hour.start_time().and_utc();

        if hour_start > *now {
            // the rest of the day has not happened yet
            break;
        }

        let hour_end = hour.end_time().and_utc().min(*now);

        match jobs_between(&hour_start, &hour_end, nodes, query, now, QUERY_TIMEOUT).await {
            Ok(hour_jobs) => jobs.extend(hour_jobs),
            Err(e) => {
                tracing::warn!(
                    "Could not read {}: {:#}. This hour is missing from the report.",
                    hour,
                    e
                );
                missing_hours = missing_hours.saturating_add(1);
            }
        }
    }

    DayRecords {
        jobs,
        missing_hours,
    }
}
