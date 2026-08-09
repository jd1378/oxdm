//! Start an existing job by id and print whatever the daemon says.
//!
//! Test scaffolding: the GUI logs a failed start and moves on, so a
//! queue that quietly starts nothing gives you nothing to read.
//!
//!     cargo run --example startjob -- <job-id>

#[tokio::main]
async fn main() {
    let Some(id) = std::env::args().nth(1) else {
        eprintln!("usage: startjob <job-id>");
        std::process::exit(2);
    };
    let id: oxdm::domain::JobId = id.parse().expect("job id");
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    if std::env::args().any(|a| a == "--pause") {
        match client.pause(id).await {
            Ok(()) => println!("paused"),
            Err(e) => println!("refused: {e}"),
        }
        return;
    }
    match client.start_job(id).await {
        Ok(()) => println!("started"),
        Err(e) => println!("refused: {e}"),
    }
}
