//! Add a download the way the Add dialog does, with or without the
//! details a probe would have filled in.
//!
//! Test scaffolding for the "add it now, name it later" path: the
//! dialog only leaves the name blank if the user submits inside the
//! probe's own round trip, which is not a race a test should be built
//! on.
//!
//!     cargo run --example addjob -- http://127.0.0.1:8088/f.bin /tmp --start

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(url), Some(dir)) = (args.next(), args.next()) else {
        eprintln!("usage: addjob <url> <save-dir> [--start]");
        std::process::exit(2);
    };
    let start = args.any(|a| a == "--start");

    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let id = client
        .add_job(oxdm::ipc_local::protocol::AddJobReq {
            url: url.parse().expect("url"),
            save_dir: dir.into(),
            // The point of the exercise: no name, no size, no digests.
            filename: None,
            referrer: None,
            headers: Default::default(),
            max_connections: None,
            proxy: None,
            auth_user: None,
            auth_password: None,
            proxy_password: None,
            cookies: None,
            category: None,
            size: None,
            checksums: Vec::new(),
        })
        .await
        .expect("add");
    println!("{id}");
    if start {
        client.start_job(id).await.expect("start");
    }
}
