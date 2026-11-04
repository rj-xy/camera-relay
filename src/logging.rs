//! Logs to the console and to the systemd journal, and logs panics instead of losing them.
//!
//! Level: `CAMERA_RELAY_LOG` (or `RUST_LOG`), such as `debug` or `camera_relay=trace`. Default `info`.

use std::backtrace::Backtrace;
use std::io::IsTerminal;
use std::panic::PanicHookInfo;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// Name the journal files the logs under: `journalctl -t camera-relay`.
pub const JOURNAL_IDENTIFIER: &str = "camera-relay";

/// Sets up the console and journal outputs. Call once, first thing in `main`.
pub fn init() {
    let filter = EnvFilter::try_from_env("CAMERA_RELAY_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Under systemd, stderr already goes to the journal: skip the console copy there.
    let console = (!stderr_is_journal()).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(std::io::stderr().is_terminal())
            .with_target(false)
            .without_time() // the journal has its own, in local time
            .compact()
    });
    let journal = tracing_journald::layer()
        .map(|layer| layer.with_syslog_identifier(JOURNAL_IDENTIFIER.to_owned()))
        .ok();
    let journal_missing = journal.is_none();

    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(journal)
        .init();

    if journal_missing {
        tracing::debug!("No systemd journal here: logging to the console only");
    }
}

/// Logs every panic with its location and a backtrace, on whichever thread it happens.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        tracing::error!(
            "🔴 Panic on thread {thread} at {}: {}\n{}",
            location(info),
            payload(info),
            Backtrace::force_capture()
        );
    }));
}

fn location(info: &PanicHookInfo<'_>) -> String {
    info.location()
        .map(|l| format!("{}:{}", l.file(), l.line()))
        .unwrap_or_else(|| "unknown place".to_owned())
}

/// The panic message, if it was a string.
pub fn payload(info: &PanicHookInfo<'_>) -> String {
    payload_text(info.payload())
}

pub fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(no message)".to_owned()
    }
}

/// systemd sets `JOURNAL_STREAM=<device>:<inode>` when stderr is the journal.
fn stderr_is_journal() -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Some((dev, ino)) = stream.to_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    let Ok(meta) = std::fs::metadata("/proc/self/fd/2") else {
        return false;
    };
    dev.parse() == Ok(meta.dev()) && ino.parse() == Ok(meta.ino())
}
