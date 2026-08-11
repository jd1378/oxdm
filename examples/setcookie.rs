//! Put a long unbroken cookie string on a job, for looking at how the
//! Cookies editor wraps it.
//!
//!     cargo run --example setcookie -- <job-id> [len]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(id) = args.next() else {
        eprintln!("usage: setcookie <job-id> [len]");
        std::process::exit(2);
    };
    let len: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(300);
    let id: oxdm::domain::JobId = id.parse().expect("job id");
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let job = client
        .job_entry(id)
        .await
        .expect("snapshot")
        .expect("job not found")
        .job;
    let mut adv = job.advanced.clone();
    adv.cookies_enabled = true;
    adv.cookie_jar = "d".repeat(len);
    client.set_job_advanced(id, adv).await.expect("set");
    println!("ok");
}
