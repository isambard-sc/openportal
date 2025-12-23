// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent_core::Config;
use crate::error::Error;
use crate::job::{Envelope, Job};
use crate::handler::{process_message, set_my_service_details};
use crate::runnable::AsyncRunnable;

use serde::{Deserialize, Serialize};

///
/// Run the filesystem service
///
pub async fn run<T>(config: Config<T>, runner: AsyncRunnable) -> Result<(), Error>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + std::fmt::Debug + Default,
{
    if config.service().name().is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()));
    }

    if config.agent() != AgentType::Filesystem {
        return Err(Error::Misconfigured(
            "Service agent is not a Filesystem".to_string(),
        ));
    }

    // pass the service details onto the handler
    // Filesystem agents are leaf nodes, so don't cascade health
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

    // run the Provider OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service()).await?;

    Ok(())
}
