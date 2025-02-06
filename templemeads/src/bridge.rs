// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::error::Error;
use crate::job::Job;
use crate::state;

use anyhow::Result;
use uuid::Uuid;

pub async fn status(job: &Uuid) -> Result<Job, Error> {
    tracing::debug!("Received status request for job: {}", job);

    match agent::portal(5).await {
        Some(portal) => {
            // get the (shared) board for the portal
            let board = match state::get(&portal).await {
                Ok(b) => b.board().await,
                Err(e) => {
                    tracing::error!("Error getting board for portal: {:?}", e);
                    return Err(Error::State(e.to_string()));
                }
            };

            // get the board from the Arc<RwLock> board - this is the
            // blocking operation
            let board = board.read().await;

            // get the job from the board
            Ok(board.get(job)?.clone())
        }
        None => {
            tracing::error!("No portal agent found");
            Err(Error::NoPortal(
                "Cannot get the job status because there is no portal agent".to_string(),
            ))
        }
    }
}

pub async fn run(command: &str) -> Result<Job, Error> {
    tracing::info!("Received command: {}", command);

    let my_name = agent::name().await;

    match agent::portal(5).await {
        Some(portal) => {
            let job = Job::parse(command, true)?;

            if job.destination().first() != portal.name() {
                tracing::error!(
                    "Job destination does not match portal name: {} != {}",
                    job.destination(),
                    portal.name()
                );
                return Err(Error::Delivery(format!(
                    "Job destination does not match portal name: {} != {}",
                    job.destination().first(),
                    portal.name(),
                )));
            }

            let job = Job::parse(
                &format!("{}.{} submit {}", my_name, portal.name(), command),
                true,
            )?;

            // use a longer duration for this job so that there is plenty of
            // time for the portal to collect the result - in reality, the
            // actual job on the system will have a much shorter lifetime,
            // e.g. 1 minute
            let job = job.set_lifetime(chrono::Duration::minutes(15));

            Ok(job.put(&portal).await?)
        }
        None => {
            tracing::error!("No portal agent found");
            Err(Error::NoPortal(
                "Cannot run the job because there is no portal agent".to_string(),
            ))
        }
    }
}
