//! Start a queue by name and print whatever the daemon says.
//!
//!     cargo run --example startqueue -- Main

#[tokio::main]
async fn main() {
    let want = std::env::args().nth(1).unwrap_or_else(|| "Main".into());
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let queues = client.snapshot().await.expect("snapshot").queues;
    let Some(q) = queues.iter().find(|q| q.name == want) else {
        eprintln!("no queue named {want}");
        std::process::exit(2);
    };
    match client.start_queue(q.id).await {
        Ok(()) => println!("started {}", q.name),
        Err(e) => println!("refused: {e}"),
    }
}
