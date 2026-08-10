//! Put a wrong checksum on a finished download and ask the daemon to
//! check it — the state where the file is complete, the phase says so,
//! and the verdict says the bytes are not what they claim.
//!
//!     cargo run --example badsum -- <job-id>

#[tokio::main]
async fn main() {
    let Some(id) = std::env::args().nth(1) else {
        eprintln!("usage: badsum <job-id>");
        std::process::exit(2);
    };
    let id: oxdm::domain::JobId = id.parse().expect("job id");
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    client
        .set_job_checksums(
            id,
            vec![oxdm::domain::Checksum {
                algo: oxdm::domain::Algo::Sha256,
                hash: "0".repeat(64),
                source: oxdm::domain::CsSource::User,
                status: oxdm::domain::CsStatus::Unverified,
                expected: None,
            }],
        )
        .await
        .expect("set checksums");
    println!("{:?}", client.verify_checksums(id).await);
}
