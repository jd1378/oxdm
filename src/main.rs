// Build oxdm.exe with the Windows GUI subsystem so launching the
// daemon, tray, or any GUI subprocess does not flash an empty console
// window. CLI subcommands (`--quit`, `--version`, `--help`, error
// messages) reattach to the parent console at startup so terminal
// invocations still see stdout / stderr.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use oxdm::{daemon, single_instance};

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
                // iced_winit dumps every window's full WindowAttributes
                // at INFO on creation; winit warns about XSETTINGS/randr
                // quirks on every X11 connect; sctk_adwaita warns about
                // unsupported tokens in GNOME's button-layout gsetting
                // (e.g. `icon:`) even though our borderless windows
                // never show its frame. All spam per-window
                // subprocesses — keep them at warn/error.
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        "info,oxdm=debug,iced_winit=warn,winit=error,sctk_adwaita=error",
                    )
                }),
        )
        .init();

    // CLI dispatch:
    //   oxdm                  → daemon (tray + downloads + IPC; no window)
    //   oxdm gui main         → main GUI subprocess
    //   oxdm gui download ID  → per-job download window subprocess
    //   oxdm --quit           → tell the running daemon to terminate
    //   oxdm --install-update → swap in a downloaded update (internal)
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("gui") => match args.next().as_deref() {
            Some("main") => oxdm::gui::windows::main::launch_main(),
            Some("download") => {
                let Some(id_str) = args.next() else {
                    eprintln!("oxdm gui download <job-id>");
                    std::process::exit(2);
                };
                let Ok(id) = id_str.parse::<oxdm::domain::JobId>() else {
                    eprintln!("invalid job id: {id_str}");
                    std::process::exit(2);
                };
                oxdm::gui::windows::download::launch_download(id);
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
                oxdm::gui::windows::properties::launch_properties(id);
            }
            Some("add") => {
                // Re-parsed inside the window (see `add::parse_args`);
                // read here only so an unusable invocation fails before
                // a window opens.
                let mut edit_id: Option<oxdm::domain::JobId> = None;
                let mut prefill_url: Option<String> = None;
                while let Some(a) = args.next() {
                    if a == "--url" {
                        prefill_url = args.next();
                    } else if a == "--staged" {
                        // The path is the window's to read and delete.
                        let _ = args.next();
                    } else if let Ok(id) = a.parse::<oxdm::domain::JobId>() {
                        edit_id = Some(id);
                    }
                }
                oxdm::gui::windows::add::launch_add(edit_id, prefill_url);
            }
            Some("settings") => {
                // --tab / --highlight-proxy are re-parsed inside the window.
                oxdm::gui::windows::settings::launch_settings();
            }
            Some("queues") => {
                oxdm::gui::windows::queues::launch_queues();
            }
            Some("power") => {
                oxdm::gui::windows::power::launch_power();
            }
            Some("about") => {
                oxdm::gui::windows::about::launch_about();
            }
            Some("batch") => {
                let Some(path) = args.next() else {
                    eprintln!("oxdm gui batch <staged-json-path>");
                    std::process::exit(2);
                };
                oxdm::gui::windows::batch::launch_batch(std::path::PathBuf::from(path));
            }
            other => {
                eprintln!("unknown gui command: {other:?}");
                std::process::exit(2);
            }
        },
        // Hidden: oxdm re-runs itself from a copy elsewhere to replace
        // the installed programs, because a running program cannot
        // replace itself. Not in --help; the app spawns it, users do
        // not — and a build whose files belong to a package manager
        // never spawns it at all, so being asked here is either a
        // mistake or someone else's idea.
        Some("--install-update") if oxdm::domain::SELF_UPDATE => oxdm::update_install::main(args),
        Some("--install-update") => {
            eprintln!(
                "this build does not update itself — it is installed and updated by \
                 your package manager"
            );
            std::process::exit(2);
        }
        Some("--install-native-host") => install_native_host(args),
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
    println!("oxdm - cross-platform download manager");
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
    println!("    oxdm gui about            Run the About window");
    println!("    oxdm --tray               Start daemon hidden (no main window)");
    println!("    oxdm --install-native-host   Register oxdm with your browsers");
    println!("        [--chromium-id ID] [--firefox-id ID] [--host-binary PATH]");
    println!("        [--db-path PATH] [--token-file PATH] [--patch-desktop] [--dry-run]");
    println!("    oxdm --quit               Tell the running daemon to exit");
    println!("    oxdm --version            Print version");
    println!("    oxdm --help               This text");
}

/// Register the native-messaging host with every browser on this
/// machine. The app does this on first run and offers it again from
/// Settings; this is the same code for people who would rather type,
/// and for the install scripts, which call it rather than keeping
/// their own copy of where a manifest goes.
fn install_native_host(mut args: impl Iterator<Item = String>) -> ! {
    let mut opts = oxdm::data::native_host::Options::default();
    let (mut chromium, mut firefox) = (Vec::new(), Vec::new());
    let path = |arg: Option<String>, flag: &str| -> std::path::PathBuf {
        match arg {
            Some(v) => std::path::PathBuf::from(v),
            None => fail(&format!("{flag} needs a value")),
        }
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chromium-id" => match args.next() {
                Some(v) => chromium.extend(v.split(',').map(str::trim).map(str::to_owned)),
                None => fail("--chromium-id needs a value"),
            },
            "--firefox-id" => match args.next() {
                Some(v) => firefox.extend(v.split(',').map(str::trim).map(str::to_owned)),
                None => fail("--firefox-id needs a value"),
            },
            "--host-binary" => opts.host_binary = Some(path(args.next(), "--host-binary")),
            "--db-path" => opts.db_path = Some(path(args.next(), "--db-path")),
            "--token-file" => opts.token_file = Some(path(args.next(), "--token-file")),
            "--patch-desktop" => opts.patch_desktop = true,
            // The flag itself is the consent; kept so the old
            // invocation still runs.
            "-y" | "--yes" => {}
            "--dry-run" => opts.dry_run = true,
            other => fail(&format!("unknown flag: {other}")),
        }
    }
    let dry_run = opts.dry_run;
    let ids = &mut opts.ids;
    // Given ids replace the shipped ones for that family only: pairing
    // a development build of the extension should not also stop the
    // published one working in the other browser.
    if !chromium.is_empty() {
        ids.chromium = chromium;
    }
    if !firefox.is_empty() {
        ids.firefox = firefox;
    }

    let report = match oxdm::data::native_host::install(&opts) {
        Ok(r) => r,
        Err(e) => fail(&e),
    };
    for entry in &report.entries {
        let verb = match &entry.outcome {
            oxdm::domain::HostOutcome::Written if dry_run => "would write",
            oxdm::domain::HostOutcome::Written => "wrote",
            oxdm::domain::HostOutcome::Unchanged => "unchanged",
            oxdm::domain::HostOutcome::Failed(e) => {
                println!("{}: FAILED: {e}", entry.browser);
                continue;
            }
        };
        println!("{}: {verb} {}", entry.browser, entry.manifest);
    }
    if report.no_browsers {
        println!("No supported browser found.");
    }
    if !report.flatpak_grants.is_empty() {
        println!();
        println!("Flatpak browsers cannot reach oxdm until you grant them the paths:");
        for cmd in &report.flatpak_grants {
            println!("    {cmd}");
        }
        if !opts.patch_desktop {
            println!();
            println!("Or rerun with --patch-desktop to splice the same grants into each");
            println!("browser's user .desktop file instead of keeping an override.");
        }
    }
    if !report.desktop_patched.is_empty() {
        println!();
        println!("Desktop files:");
        for line in &report.desktop_patched {
            println!("    {line}");
        }
    }
    std::process::exit(if report.failures() > 0 { 1 } else { 0 });
}

fn fail(message: &str) -> ! {
    eprintln!("oxdm --install-native-host: {message}");
    std::process::exit(2);
}

fn run_daemon() {
    let guard = match single_instance::acquire() {
        Ok(single_instance::InstanceOutcome::Primary(g)) => g,
        Ok(single_instance::InstanceOutcome::AlreadyRunning) => {
            tracing::info!("oxdm daemon already running, surfacing main window");
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
