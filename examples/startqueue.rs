//! Start a queue by name and print whatever the daemon says.
//!
//!     cargo run --example startqueue -- Main

#[tokio::main]
async fn main() {
    let want = std::env::args().nth(1).unwrap_or_else(|| "Main".into());
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let queues = client.snapshot().await.expect("snapshot").queues;
    let Some(q) = queues.iter().find(|q| q.name == want) else {
        eprintln!("no queue named {want}");
        std::process::exit(2);
    };
    // What the toolbar reads: is the queue itself running, and what is
    // running inside it.
    if std::env::args().any(|a| a == "--status") {
        let snap = client.snapshot().await.expect("snapshot");
        println!("queue_active={}", snap.active_queues.contains(&q.id));
        for j in snap.jobs.iter().filter(|j| j.queue_id == q.id) {
            println!("{} {:?} {:?}", j.id, j.filename, j.status.phase);
        }
        return;
    }
    if std::env::args().any(|a| a == "--stop") {
        match client.stop_queue(q.id).await {
            Ok(()) => println!("stopped {}", q.name),
            Err(e) => println!("refused: {e}"),
        }
        return;
    }
    match client.start_queue(q.id).await {
        Ok(()) => println!("started {}", q.name),
        Err(e) => println!("refused: {e}"),
    }
}
