// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent_core::Config;
use crate::error::Error;
use crate::handler::{process_message, set_my_service_details};
use crate::job::{Envelope, Job};
use crate::runnable::AsyncRunnable;

///
/// Run the scheduler service
///
pub async fn run(config: Config, runner: AsyncRunnable) -> Result<(), Error> {
    if config.service().name().is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()));
    }

    if config.agent() != AgentType::Scheduler {
        return Err(Error::Misconfigured(
            "Service agent is not a Scheduler".to_string(),
        ));
    }

    // pass the service details onto the handler
    set_my_service_details(
        &config.service().name(),
        &config.agent(),
        Some(runner),
        false,
    )
    .await?;

    if let Some(one_shot_commands) = config.one_shot_commands() {
        for one_shot_command in one_shot_commands {
            tracing::info!("Executing one-shot command: {}", one_shot_command);

            let job = Job::parse(
                format!("oneshot.{} {}", config.service().name(), one_shot_command).as_str(),
                false,
            )?
            .pending()?;

            let envelope = Envelope::new(
                &config.service().name(),
                &config.service().name(),
                "one-shot",
                &job,
            );

            let job = runner(envelope).await?;

            let result = serde_json::from_str::<serde_json::Value>(&job.result_json()?);

            // now write this out as pretty-printed JSON
            match result {
                Ok(json) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json).unwrap_or_else(|_| {
                            "Failed to serialize result as pretty-printed JSON".to_string()
                        })
                    );
                }
                Err(_) => {
                    println!("{}", job.result_json()?);
                }
            }
        }

        return Ok(());
    }

    // run the OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service()).await?;

    Ok(())
}
