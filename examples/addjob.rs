//! Add a download the way the Add dialog does, with or without the
//! details a probe would have filled in.
//!
//! Test scaffolding for the "add it now, name it later" path: the
//! dialog only leaves the name blank if the user submits inside the
//! probe's own round trip, which is not a race a test should be built
//! on.
//!
//!     cargo run --example addjob -- http://127.0.0.1:8088/f.bin /tmp --start
//!             [--referer https://example.com/page] [--ua "curl/8"]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(url), Some(dir)) = (args.next(), args.next()) else {
        eprintln!("usage: addjob <url> <save-dir> [--start]");
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();
    let start = rest.iter().any(|a| a == "--start");
    // Milliseconds between adding and starting, for reaching the window
    // where a background probe is still in flight when the user hits
    // Resume.
    let start_after: u64 = rest
        .iter()
        .position(|a| a == "--start-after")
        .and_then(|i| rest.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // A job with credentials is never probed in the background, which
    // is the only way to reach the run's own name-resolution path
    // without racing a probe that answers first.
    let user = rest.iter().any(|a| a == "--auth").then(|| "u".to_owned());
    // A size the caller claims to have probed, for exercising decisions
    // that turn on how big the file is.
    let size: Option<u64> = rest
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| rest.get(i + 1))
        .and_then(|v| v.parse().ok());

    // Identification, the way a browser capture would arrive with it.
    let flag = |name: &str| -> Option<String> {
        rest.iter()
            .position(|a| a == name)
            .and_then(|i| rest.get(i + 1))
            .cloned()
    };
    let referrer = flag("--referer").map(|r| r.parse().expect("referer url"));
    let mut headers: indexmap::IndexMap<String, String> = Default::default();
    if let Some(ua) = flag("--ua") {
        headers.insert("User-Agent".to_owned(), ua);
    }

    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let id = client
        .add_job(oxdm::ipc_local::protocol::AddJobReq {
            url: url.parse().expect("url"),
            save_dir: dir.into(),
            // The point of the exercise: no name, no size, no digests.
            filename: None,
            referrer,
            headers,
            max_connections: None,
            proxy: None,
            auth_user: user,
            auth_password: None,
            proxy_password: None,
            cookies: None,
            category: None,
            size,
            checksums: Vec::new(),
        })
        .await
        .expect("add");
    println!("{id}");
    if start || start_after > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(start_after)).await;
        client.start_job(id).await.expect("start");
    }
}
