//! Flip a boolean setting on the running daemon, the way the Settings
//! window does.
//!
//! Test scaffolding: driving a toggle by synthetic click is fragile —
//! the row has to be scrolled into view and the window found by name —
//! and a test of what the *daemon* does with a setting should not
//! depend on any of that.
//!
//!     cargo run --example setflag -- forget_moved_files false

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(key), Some(value)) = (args.next(), args.next()) else {
        eprintln!("usage: setflag <setting-key> <true|false>");
        std::process::exit(2);
    };
    let on: bool = match value.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("setflag: value must be true or false");
            std::process::exit(2);
        }
    };

    let client = oxdm::ipc_local::Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .expect("no daemon");
    let mut settings = client.snapshot().await.expect("snapshot").settings;
    match key.as_str() {
        "forget_moved_files" => settings.forget_moved_files = on,
        other => {
            eprintln!("setflag: unknown key {other}");
            std::process::exit(2);
        }
    }
    client.update_settings(settings).await.expect("update");
    println!("{key} = {on}");
}
