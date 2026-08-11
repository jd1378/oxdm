//! Ask the daemon for a fresh pairing code without saving it.
//!
//!     cargo run --example mintcode

#[tokio::main]
async fn main() {
    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    println!("{:?}", client.mint_ext_token().await);
}
