// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use paddington;
use tokio;

#[tokio::main]
async fn main() {
    let config = paddington::config::load().await.unwrap_or_else(|err| {
        panic!("Error loading config: {:?}", err);
    });

    println!("Loaded config: {:?}", config);

    paddington::server::run(config).await;
}
