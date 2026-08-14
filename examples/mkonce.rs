//! Create a queue with a one-off schedule, for the status-bar line
//! that only appears while such a queue is selected.
//!
//!     cargo run --example mkonce -- "Tonight" 26   # hours from now

#[tokio::main]
async fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "Once".into());
    let hours: i64 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(26);
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let q = oxdm::domain::Queue {
        id: oxdm::domain::QueueId::new(),
        name,
        builtin: false,
        job_ids: Vec::new(),
        schedule: oxdm::domain::QueueSchedule::Once {
            start: chrono::Local::now() + chrono::Duration::hours(hours),
            stop: None,
        },
        on_start: Vec::new(),
        on_finish: Vec::new(),
        max_concurrent: None,
        stop_on_error: false,
        color: None,
    };
    client.upsert_queue(q).await.expect("upsert queue");
}
