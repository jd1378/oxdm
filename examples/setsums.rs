//! Put an arbitrary set of checksums on a job, for the states a real
//! run reaches only by luck — a verified row beside an unverified one.
//!
//!     cargo run --example setsums -- <job-id> [--no-verify] sha256:<hex>

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(id) = args.next() else {
        eprintln!("usage: setsums <job-id> <algo:hash>...");
        std::process::exit(2);
    };
    let id: oxdm::domain::JobId = id.parse().expect("job id");
    let specs: Vec<String> = args.collect();
    let verify = !specs.iter().any(|a| a == "--no-verify");
    let rows: Vec<oxdm::domain::Checksum> = specs
        .into_iter()
        .filter(|a| a != "--no-verify")
        .map(|spec| {
            let (algo, hash) = spec.split_once(':').expect("algo:hash");
            let algo = match algo {
                "md5" => oxdm::domain::Algo::Md5,
                "sha1" => oxdm::domain::Algo::Sha1,
                "sha256" => oxdm::domain::Algo::Sha256,
                "sha384" => oxdm::domain::Algo::Sha384,
                other => panic!("unknown algorithm {other}"),
            };
            oxdm::domain::Checksum {
                algo,
                hash: hash.to_owned(),
                source: oxdm::domain::CsSource::User,
                status: oxdm::domain::CsStatus::Unverified,
                expected: None,
            }
        })
        .collect();
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    client.set_job_checksums(id, rows).await.expect("set");
    if verify {
        println!("{:?}", client.verify_checksums(id).await);
    }
}
