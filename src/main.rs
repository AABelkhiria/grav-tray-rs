#[cfg(target_os = "macos")]
mod macos_app;

use grav_tray_rs::launch_agent;
use std::process::ExitCode;

fn main() -> ExitCode {
    match command() {
        Command::Run => run(),
        Command::Install => report(
            launch_agent::install_and_start(),
            "Installed and started Grav Tray.",
        ),
        Command::Uninstall => report(
            launch_agent::uninstall_and_stop(),
            "Stopped Grav Tray and removed its LaunchAgent.",
        ),
        Command::Diagnose => diagnose(),
        Command::Help => {
            print_help();
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("grav-tray-rs {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Invalid(argument) => {
            eprintln!("Unknown argument: {argument}\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Run,
    Install,
    Uninstall,
    Diagnose,
    Help,
    Version,
    Invalid(String),
}

fn command() -> Command {
    match std::env::args().nth(1).as_deref() {
        None => Command::Run,
        Some("--install") => Command::Install,
        Some("--uninstall") => Command::Uninstall,
        Some("--diagnose") => Command::Diagnose,
        Some("-h" | "--help") => Command::Help,
        Some("-V" | "--version") => Command::Version,
        Some(argument) => Command::Invalid(argument.to_owned()),
    }
}

#[cfg(target_os = "macos")]
fn run() -> ExitCode {
    macos_app::run();
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "macos"))]
fn run() -> ExitCode {
    eprintln!("grav-tray-rs is a native macOS menu bar application.");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn report(result: Result<(), String>, success_message: &str) -> ExitCode {
    match result {
        Ok(()) => {
            println!("{success_message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn report(_result: Result<(), String>, _success_message: &str) -> ExitCode {
    eprintln!("grav-tray-rs is a native macOS menu bar application.");
    ExitCode::FAILURE
}

fn diagnose() -> ExitCode {
    use grav_tray_rs::quota::{candidate_http_ports, enabled_buckets, fetch_quota};

    let Some(home) = dirs::home_dir() else {
        eprintln!("Home directory: not found");
        return ExitCode::FAILURE;
    };
    let log_directory = home.join(".gemini").join("antigravity-cli").join("log");
    println!("Log directory: {}", log_directory.display());
    println!("Exists: {}", log_directory.is_dir());

    let ports = candidate_http_ports(&home);
    if ports.is_empty() {
        eprintln!("Candidate HTTP ports: none");
        return ExitCode::FAILURE;
    }
    println!(
        "Candidate HTTP ports: {}",
        ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );

    match fetch_quota(&home, None) {
        Ok((summary, port)) => {
            println!("Connected port: {port}");
            println!("Quota groups: {}", summary.groups.len());
            for (group, bucket) in enabled_buckets(&summary) {
                let remaining = bucket
                    .percent()
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "unknown".to_owned());
                println!(
                    "  {} — {}: {remaining}",
                    group.display_name, bucket.display_name
                );
            }
            println!("Diagnosis: OK");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Diagnosis: FAILED");
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "Grav Tray — Antigravity quota in the macOS menu bar

Usage:
  grav-tray-rs              Run in the foreground
  grav-tray-rs --install    Install a LaunchAgent and start Grav Tray
  grav-tray-rs --uninstall  Stop Grav Tray and remove its LaunchAgent
  grav-tray-rs --diagnose   Test agy log discovery and quota access
  grav-tray-rs --version    Print the version
  grav-tray-rs --help       Show this help"
    );
}
