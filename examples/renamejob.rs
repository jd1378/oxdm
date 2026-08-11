//! Rename a job the way the Properties dialog does, and print what the
//! daemon says.
//!
//! Test scaffolding for the one-name-one-download rule: an add gets
//! numbered, a rename gets refused, and the refusal is what a headless
//! run can check.
//!
//!     cargo run --example renamejob -- <job-id> <new-file-name>

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(id), Some(name)) = (args.next(), args.next()) else {
        eprintln!("usage: renamejob <job-id> <new-file-name>");
        std::process::exit(2);
    };
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
    match client
        .set_job_source(id, job.url.clone(), job.save_dir.clone(), Some(name))
        .await
    {
        Ok(()) => println!("renamed"),
        Err(e) => println!("refused: {e}"),
    }
}
