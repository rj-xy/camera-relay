//! Command line: parsing, and the client commands that talk to the running app over D-Bus.

use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use zbus::fdo;
use zbus::proxy::CacheProperties;

use crate::dbus::{BUS_NAME, CameraRelayProxy};
use crate::model::Adjustments;

#[derive(Parser)]
#[command(
    version,
    about = "A virtual webcam for Linux with live picture adjustments and profiles"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the app (the default when no command is given)
    Run {
        /// Turn the relay on straight away
        #[arg(long)]
        start: bool,
        /// Don't show a tray icon
        #[arg(long)]
        no_tray: bool,
    },
    /// Turn the relay on (starts the app if it isn't running)
    Start,
    /// Turn the relay off
    Stop,
    /// Turn the relay on or off (starts the app if it isn't running)
    Toggle,
    /// Show whether the relay runs, the profile and its settings
    Status,
    /// Open the settings window (starts the app if it isn't running)
    Settings,
    /// List profiles, or switch to NAME (a new name starts as a copy of the current profile)
    Profile { name: Option<String> },
    /// Rename a profile
    RenameProfile { from: String, to: String },
    /// Delete a profile (the active one switches to default first)
    DeleteProfile { name: String },
    /// Change a setting of the active profile, live and saved
    #[command(after_help = format!("Settings: {}", Adjustments::KEYS.join(", ")))]
    Set { key: String, value: String },
    /// Start on login: show the setting, or change it
    Autostart {
        #[arg(value_enum)]
        mode: Option<crate::autostart::Mode>,
    },
    /// Replace this program with the latest release from GitHub
    Update,
}

/// Runs a client command against the running app.
pub fn run(command: Command) -> Result<()> {
    // Rust ignores SIGPIPE, so `camera-relay status | head -1` would panic on the closed pipe.
    // A command-line tool should just end there, like `ls` does. (The app itself keeps ignoring it.)
    // SAFETY: called before any other thread starts.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    zbus::block_on(run_async(command))
}

async fn run_async(command: Command) -> Result<()> {
    let connection = zbus::Connection::session()
        .await
        .context("can't reach the D-Bus session bus")?;
    let launch = matches!(
        command,
        Command::Start | Command::Toggle | Command::Settings
    );
    if !app_is_running(&connection).await? {
        if !launch {
            bail!("camera-relay isn't running. Start it with: camera-relay");
        }
        launch_app().await?;
    }
    let relay = CameraRelayProxy::builder(&connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    match command {
        Command::Run { .. } | Command::Autostart { .. } | Command::Update => {
            unreachable!("handled by main")
        }
        Command::Start => {
            relay.start().await.map_err(message)?;
            println!("🟢 Camera relay on");
        }
        Command::Stop => {
            relay.stop().await.map_err(message)?;
            println!("🔴 Camera relay off");
        }
        Command::Toggle => {
            let on = relay.toggle().await.map_err(message)?;
            println!(
                "{}",
                if on {
                    "🟢 Camera relay on"
                } else {
                    "🔴 Camera relay off"
                }
            );
        }
        Command::Status => {
            let state = if relay.running().await? {
                "🟢 Relay on"
            } else {
                "🔴 Relay off"
            };
            println!("{state} · profile {}", relay.active_profile().await?);
            let error = relay.last_error().await?;
            if !error.is_empty() {
                println!("🟠 Last error: {error}");
            }
            println!(
                "\n{}",
                relay.adjustments().await.map_err(message)?.trim_end()
            );
        }
        Command::Settings => relay.show_settings().await.map_err(message)?,
        Command::Profile { name: None } => {
            let active = relay.active_profile().await?;
            for name in relay.profiles().await? {
                println!("{} {name}", if name == active { "🟢" } else { "  " });
            }
        }
        Command::Profile { name: Some(name) } => {
            let created = relay.use_profile(&name).await.map_err(message)?;
            let note = if created {
                " (new, copied from the previous profile)"
            } else {
                ""
            };
            println!("🟢 Profile {name}{note}");
        }
        Command::RenameProfile { from, to } => {
            relay.rename_profile(&from, &to).await.map_err(message)?;
            println!("🟢 Profile {from} renamed to {to}");
        }
        Command::DeleteProfile { name } => {
            relay.delete_profile(&name).await.map_err(message)?;
            println!("🟢 Profile {name} deleted");
        }
        Command::Set { key, value } => {
            relay.set(&key, &value).await.map_err(message)?;
            println!(
                "🟢 {key} = {value} (profile {})",
                relay.active_profile().await?
            );
        }
    }
    Ok(())
}

async fn app_is_running(connection: &zbus::Connection) -> Result<bool> {
    let bus = fdo::DBusProxy::new(connection).await?;
    Ok(bus.name_has_owner(BUS_NAME.try_into()?).await?)
}

/// Starts `camera-relay run` in the background and waits for it on the bus.
async fn launch_app() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("can't find my own executable")?;
    Process::new(exe)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0) // so Ctrl+C in this terminal doesn't reach it
        .spawn()
        .context("can't start the app")?;
    eprintln!("🌱 Started the camera-relay app");

    let connection = zbus::Connection::session().await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if app_is_running(&connection).await? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("the app didn't come up within 5 s: see journalctl -t camera-relay")
}

/// The app's own error text, without the D-Bus error name in front.
fn message(e: zbus::Error) -> anyhow::Error {
    match e {
        zbus::Error::FDO(e) => match *e {
            fdo::Error::Failed(m) => anyhow!(m),
            e => anyhow!(e),
        },
        zbus::Error::MethodError(_, Some(m), _) => anyhow!(m),
        e => anyhow!(e),
    }
}
