//! Put a wrong checksum on a finished download and ask the daemon to
//! check it — the state where the file is complete, the phase says so,
//! and the verdict says the bytes are not what they claim. `--clear`
//! then takes the offending row away again, the way the Properties
//! dialog's delete button does.
//!
//!     cargo run --example badsum -- <job-id> [--clear]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(id) = args.next() else {
        eprintln!("usage: badsum <job-id> [--clear]");
        std::process::exit(2);
    };
    let clear = args.any(|a| a == "--clear");
    let id: oxdm::domain::JobId = id.parse().expect("job id");
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let rows = if clear {
        Vec::new()
    } else {
        vec![oxdm::domain::Checksum {
            algo: oxdm::domain::Algo::Sha256,
            hash: "0".repeat(64),
            source: oxdm::domain::CsSource::User,
            status: oxdm::domain::CsStatus::Unverified,
            expected: None,
        }]
    };
    client.set_job_checksums(id, rows).await.expect("set");
    if !clear {
        println!("{:?}", client.verify_checksums(id).await);
    }
}
