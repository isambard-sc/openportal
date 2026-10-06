// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! Reads per-user, per-day node-hour usage out of the `user_daily_usage`
//! table that `nodehours-tracker` populates, and shapes it into the same
//! `ProjectUsageReport`/`DailyProjectUsageReport` structures `op-slurm`
//! answers `get_local_usage_report` with.
//!
//! `node_hours` is already the combined figure: `nodehours-tracker` folds
//! CPU, GPU, memory and pod-slot consumption (each as a fraction of what a
//! node offers) into it, so this agent does no per-resource arithmetic.
//!
//! This agent only ever reads from this table (a read-only DB role), and
//! nothing here writes to it or to the `Project`/`ProjectUser` CRs - usage
//! history is expected to outlive the user/project it was recorded against,
//! same as every other agent in OpenPortal.
//!

use anyhow::Context;
use greatwestern::grammar::{DateRange, ProjectMapping};
use greatwestern::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};
use once_cell::sync::OnceCell;
use secrecy::{ExposeSecret, SecretString};
use templemeads::Error;
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls};

static DB: OnceCell<Mutex<(SecretString, Client)>> = OnceCell::new();

/// Seconds in one hour - `node_hours` is stored as a fractional number of
/// hours, but `Usage` is exchanged with peers in seconds.
const SECONDS_PER_HOUR: f64 = 3600.0;

/// `node_hours` is `numeric(12,4)` - tokio-postgres has no built-in mapping
/// for `NUMERIC` to a Rust type, so it's cast to `float8` in the query
/// itself rather than pulling in a decimal crate for one column.
const USAGE_QUERY: &str = "SELECT username, node_hours::float8 FROM user_daily_usage \
                            WHERE project = $1 AND usage_date = $2";

///
/// Connect to the usage-tracking database, and remember both the connection
/// and the DSN (so a dropped connection can be silently re-established) for
/// the lifetime of this process.
///
/// Not yet using TLS - this assumes the database is reachable only from
/// inside the cluster's own network. Worth revisiting with
/// `tokio-postgres-rustls` if that assumption stops holding.
///
pub async fn connect(db_url: &SecretString) -> Result<(), Error> {
    let client = connect_once(db_url).await?;

    DB.set(Mutex::new((db_url.clone(), client)))
        .map_err(|_| Error::Bug("Usage database has already been connected".to_string()))?;

    Ok(())
}

async fn connect_once(db_url: &SecretString) -> Result<Client, Error> {
    let (client, connection) = tokio_postgres::connect(db_url.expose_secret(), NoTls)
        .await
        .with_context(|| "Could not connect to the usage-tracking database")?;

    // The connection object drives the actual socket I/O and has to be
    // polled somewhere for the client to make progress - see tokio-postgres'
    // own docs. Its own errors are reported here since nothing awaits it.
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("Usage database connection closed: {}", e);
        }
    });

    Ok(client)
}

///
/// Build a `ProjectUsageReport` covering `dates` for `project`, one row of
/// `user_daily_usage` per user per day.
///
pub async fn get_usage_report(
    project: &ProjectMapping,
    dates: &DateRange,
) -> Result<ProjectUsageReport, Error> {
    let db = DB
        .get()
        .ok_or_else(|| Error::Bug("Usage database has not been connected".to_string()))?;

    let mut report = ProjectUsageReport::new(project.project());
    let now = chrono::Utc::now();

    for day in dates.days() {
        let mut guard = db.lock().await;
        let rows = match guard
            .1
            .query(USAGE_QUERY, &[&project.local_group(), &day.to_chrono()])
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                // The connection this client was built from may have dropped
                // (e.g. the database restarted) - reconnect once and retry,
                // rather than leaving every later report permanently broken.
                tracing::warn!("Usage database query failed ({}), reconnecting", e);
                let db_url = guard.0.clone();
                guard.1 = connect_once(&db_url).await?;
                guard
                    .1
                    .query(USAGE_QUERY, &[&project.local_group(), &day.to_chrono()])
                    .await
                    .with_context(|| {
                        format!(
                            "Could not query usage for {} on {}",
                            project,
                            day.to_chrono()
                        )
                    })?
            }
        };
        drop(guard);

        let mut daily = DailyProjectUsageReport::default();

        for row in rows {
            let username: String = row.get(0);
            let node_hours: f64 = row.get(1);
            let seconds = (node_hours.max(0.0) * SECONDS_PER_HOUR).round() as u64;

            daily.set_usage(&username, Usage::new(seconds));
        }

        // Matches `op-slurm`'s own rule: a day is only ever marked complete
        // once it has fully ended - `nodehours-tracker` may still be
        // sampling today's figures, so today's row (and any day still to
        // come) stays provisional.
        if day.day().end_time().and_utc() < now {
            daily.set_complete();
        }

        report.set_report(&day, &daily);
    }

    Ok(report)
}
