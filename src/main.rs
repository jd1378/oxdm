// Build oxdm.exe with the Windows GUI subsystem so launching the
// daemon, tray, or any GUI subprocess does not flash an empty console
// window. CLI subcommands (`--quit`, `--version`, `--help`, error
// messages) reattach to the parent console at startup so terminal
// invocations still see stdout / stderr.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use oxdm::{daemon, single_instance, ui};

#[cfg(target_os = "windows")]
fn attach_parent_console() {
    // ATTACH_PARENT_PROCESS = u32::MAX (-1). Fails harmlessly when
    // launched from Explorer / a shortcut (no parent console), in
    // which case stdout / stderr stay /dev/null-equivalent — exactly
    // what we want for the tray launch path.
    unsafe {
        let _ = windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
    }
}

#[cfg(not(target_os = "windows"))]
fn attach_parent_console() {}

fn main() {
    attach_parent_console();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,oxdm=debug")),
        )
        .init();

    // CLI dispatch:
    //   oxdm                  → daemon (tray + downloads + IPC; no window)
    //   oxdm gui main         → main GUI subprocess
    //   oxdm gui download ID  → per-job download window subprocess
    //   oxdm --quit           → tell the running daemon to terminate
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("gui") => match args.next().as_deref() {
            Some("main") => ui::launch_main(),
            Some("download") => {
                let Some(id_str) = args.next() else {
                    eprintln!("oxdm gui download <job-id>");
                    std::process::exit(2);
                };
                let Ok(id) = id_str.parse::<oxdm::domain::JobId>() else {
                    eprintln!("invalid job id: {id_str}");
                    std::process::exit(2);
                };
                ui::launch_download(id);
            }
            Some("properties") => {
                let Some(id_str) = args.next() else {
                    eprintln!("oxdm gui properties <job-id>");
                    std::process::exit(2);
                };
                let Ok(id) = id_str.parse::<oxdm::domain::JobId>() else {
                    eprintln!("invalid job id: {id_str}");
                    std::process::exit(2);
                };
                ui::launch_properties(id);
            }
            Some("add") => {
                let mut edit_id: Option<oxdm::domain::JobId> = None;
                let mut prefill_url: Option<String> = None;
                while let Some(a) = args.next() {
                    if a == "--url" {
                        prefill_url = args.next();
                    } else if let Ok(id) = a.parse::<oxdm::domain::JobId>() {
                        edit_id = Some(id);
                    }
                }
                ui::launch_add(edit_id, prefill_url);
            }
            Some("settings") => {
                use oxdm::ui::dialogs::settings::SettingsTab;
                let mut tab: Option<SettingsTab> = None;
                let mut highlight_proxy = false;
                while let Some(a) = args.next() {
                    match a.as_str() {
                        "--tab" => {
                            tab = args.next().and_then(|s| match s.as_str() {
                                "general" => Some(SettingsTab::General),
                                "downloads" => Some(SettingsTab::Downloads),
                                "categories" => Some(SettingsTab::Categories),
                                "network" => Some(SettingsTab::Network),
                                "browser" => Some(SettingsTab::Browser),
                                "notifications" => Some(SettingsTab::Notifications),
                                "advanced" => Some(SettingsTab::Advanced),
                                "about" => Some(SettingsTab::About),
                                _ => None,
                            });
                        }
                        "--highlight-proxy" => highlight_proxy = true,
                        _ => {}
                    }
                }
                ui::launch_settings(tab, highlight_proxy);
            }
            Some("queues") => {
                ui::launch_queues();
            }
            Some("batch") => {
                let Some(path) = args.next() else {
                    eprintln!("oxdm gui batch <staged-json-path>");
                    std::process::exit(2);
                };
                ui::launch_batch(std::path::PathBuf::from(path));
            }
            other => {
                eprintln!("unknown gui command: {other:?}");
                std::process::exit(2);
            }
        },
        Some("--quit") => quit_remote(),
        Some("--tray") => run_daemon_tray(),
        Some("--version" | "-V") => {
            println!("oxdm {}", env!("CARGO_PKG_VERSION"));
        }
        Some("--help" | "-h") => {
            print_help();
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            print_help();
            std::process::exit(2);
        }
        None => run_daemon(),
    }
}

fn print_help() {
    println!("oxdm — cross-platform download manager");
    println!();
    println!("USAGE:");
    println!("    oxdm                      Start (or surface) the daemon + main window");
    println!("    oxdm gui main             Run the main GUI subprocess");
    println!("    oxdm gui download <ID>    Run a per-job download window subprocess");
    println!("    oxdm gui properties <ID>  Run a per-job Properties window subprocess");
    println!("    oxdm gui add [<ID>]       Run the Add Download dialog (edits ID when set)");
    println!("    oxdm gui settings         Run the Settings window");
    println!("    oxdm gui queues           Run the Queues & scheduling window");
    println!("    oxdm gui batch <PATH>     Run the batch-capture triage dialog");
    println!("    oxdm --tray               Start daemon hidden (no main window)");
    println!("    oxdm --quit               Tell the running daemon to exit");
    println!("    oxdm --version            Print version");
    println!("    oxdm --help               This text");
}

fn run_daemon() {
    let guard = match single_instance::acquire() {
        Ok(single_instance::InstanceOutcome::Primary(g)) => g,
        Ok(single_instance::InstanceOutcome::AlreadyRunning) => {
            tracing::info!("oxdm daemon already running — surfacing main window");
            ask_daemon_to_open_main();
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "single-instance check failed; continuing");
            daemon::run();
            return;
        }
    };
    daemon::run_with_instance(guard);
}

fn run_daemon_tray() {
    let guard = match single_instance::acquire() {
        Ok(single_instance::InstanceOutcome::Primary(g)) => g,
        Ok(single_instance::InstanceOutcome::AlreadyRunning) => {
            // Daemon already running — `--tray` is a no-op surface
            // request: don't pop the window.
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "single-instance check failed; continuing");
            daemon::run_tray();
            return;
        }
    };
    daemon::run_with_instance_tray(guard);
}

fn ask_daemon_to_open_main() {
    use oxdm::ipc_local::Client;
    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        match Client::connect_retry(std::time::Duration::from_secs(1)).await {
            Ok(c) => {
                let _ = c.open_main_window().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not reach daemon to open main");
            }
        }
    });
}

fn quit_remote() {
    use oxdm::ipc_local::Client;
    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async {
        match Client::connect_retry(std::time::Duration::from_secs(1)).await {
            Ok(c) => {
                let _ = c.daemon_quit().await;
            }
            Err(_) => {
                eprintln!("no oxdm daemon running");
            }
        }
    });
}
