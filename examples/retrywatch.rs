//! Ask the daemon to start the filesystem watcher again — what the
//! warning dialog does after the user raises the kernel limit, without
//! the authentication prompt in the way.
//!
//!     cargo run --example retrywatch

#[tokio::main]
async fn main() {
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    println!("limit before: {:?}", client.watch_limit().await);
    client.retry_file_watch().await.expect("retry");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    println!("limit after:  {:?}", client.watch_limit().await);
}
