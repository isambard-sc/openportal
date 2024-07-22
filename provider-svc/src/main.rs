// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use paddington;
use tokio;

#[tokio::main]
async fn main() {
    paddington::client::run().await;
}
