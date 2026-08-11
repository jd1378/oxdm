//! Pause, then resume, a running job, for exercising the run-time
//! accounting that the completion page divides bytes by.
//!
//!     cargo run --example pausejob -- <job-id> [hold-secs]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(id) = args.next() else {
        eprintln!("usage: pausejob <job-id> [hold-secs]");
        std::process::exit(2);
    };
    let hold: u64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(8);
    let id: oxdm::domain::JobId = id.parse().expect("job id");
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    client.pause(id).await.expect("pause");
    println!("paused, holding {hold}s");
    tokio::time::sleep(std::time::Duration::from_secs(hold)).await;
    client.start_job(id).await.expect("resume");
    println!("resumed");
}
