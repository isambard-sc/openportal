use paddington;
use tokio;

#[tokio::main]
async fn main() {
    paddington::run_client().await;
}
