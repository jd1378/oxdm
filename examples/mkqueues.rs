//! Create N queues, for states that only show up with more of them
//! than fit on screen at once.
//!
//!     cargo run --example mkqueues -- 10

#[tokio::main]
async fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    for i in 1..=n {
        let q = oxdm::domain::Queue {
            id: oxdm::domain::QueueId::new(),
            name: format!("Queue {i}"),
            builtin: false,
            schedule: Default::default(),
            on_start: Vec::new(),
            on_finish: Vec::new(),
            max_concurrent: None,
            stop_on_error: false,
            color: None,
        };
        client.upsert_queue(q).await.expect("upsert queue");
    }
}
