// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

pub async fn run() {
    let url = "ws://localhost:8080/socket";

    // connect to socket
    if let Ok((mut socket, _)) = connect_async(url).await {
        println!("Successfully connected to the WebSocket");

        for i in 0..10 {
            // create message
            let message = Message::from(format!("Message {}", i));

            // send message
            if let Err(e) = socket.send(message).await {
                eprintln!("Error sending message: {:?}", e);
            }

            // recieve response
            if let Some(Ok(response)) = socket.next().await {
                println!("{response}");
            }
        }
    } else {
        eprintln!("Failed to connect to the WebSocket");
    }
}
