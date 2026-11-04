//! camera-relay: a virtual webcam for Linux. See docs/architecture.md.

mod autostart;
mod cli;
mod config;
mod controller;
mod daemon;
mod dbus;
mod icon;
mod logging;
mod model;
mod pipeline;
mod plugins;
mod settings;
mod tray;
mod ui;
mod update;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};

fn main() -> ExitCode {
    logging::init();
    logging::install_panic_hook();

    let cli = Cli::parse();
    // The panic hook has logged any panic already; this only turns it into a clean exit code.
    let result = catch_unwind(AssertUnwindSafe(|| match cli.command {
        None => daemon::run(false, true),
        Some(Command::Run { start, no_tray }) => daemon::run(start, !no_tray),
        Some(Command::Autostart { mode }) => autostart::run(mode),
        Some(Command::Update) => update::run(),
        Some(command) => cli::run(command),
    }));

    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            tracing::error!("🔴 {e:#}");
            ExitCode::FAILURE
        }
        Err(_) => ExitCode::from(101),
    }
}
