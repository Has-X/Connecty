#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod ipc;
mod journal;
mod manager;
mod model;
mod platform;
mod setup;
mod ssh;
mod startup;
mod state;
mod supervisor;
use model::*;
use std::path::PathBuf;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

fn main() {
    if let Err(e) = entry() {
        eprintln!("{e}");
        if !std::env::args().any(|a| a == "--ctl") {
            let message: String = e.to_string().chars().take(1024).collect();
            if let Ok(dir) = config::data_dir() {
                let _ = std::fs::write(dir.join("agent-error.log"), message);
            }
        }
        std::process::exit(1);
    }
}
fn entry() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(op) = args.get(1).filter(|s| {
        matches!(
            s.as_str(),
            "--setup-prepare" | "--setup-finish" | "--setup-remove" | "--setup-rollback"
        )
    }) {
        return setup::run(op, args.get(2).map(String::as_str));
    }
    if std::env::args().any(|a| a == "--watchdog") {
        return supervisor::run();
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let custom = args
        .iter()
        .position(|s| s == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let profile_dir = args
        .iter()
        .position(|s| s == "--profile-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    if custom.is_some() && profile_dir.is_some() {
        return Err("Choose only one data directory option".into());
    }
    let name = platform::pipe_name(custom.as_deref())?;
    if let Some(i) = args.iter().position(|s| s == "--ctl") {
        let command: serde_json::Value =
            serde_json::from_str(args.get(i + 1).ok_or("Missing JSON command")?)?;
        let response = ipc::control(&name, command).await?;
        println!("{}", serde_json::to_string(&response)?);
        if response["ok"] != true {
            std::process::exit(2);
        }
        return Ok(());
    }
    let dir = match custom.clone().or(profile_dir).or_else(startup::profile_dir) {
        Some(path) => path,
        None => config::data_dir()?,
    };
    std::fs::create_dir_all(&dir)?;
    // Windows can merge redirected files into an otherwise unredirected folder.
    // Resolve the existing profile file first, so a scheduled launch sees it too.
    let existing = std::fs::canonicalize(dir.join("profiles.json")).ok();
    let dir = match existing.as_ref().filter(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("profiles.json"))
    }) {
        Some(file) => file
            .parent()
            .ok_or("Invalid profile file location")?
            .to_path_buf(),
        None => std::fs::canonicalize(dir)?,
    };
    let security = platform::Security::new()?;
    // Own the pipe before starting connections or publishing a tray icon.
    let listener = match ipc::create(&name, true, &security) {
        Ok(listener) => listener,
        Err(error) => {
            // A duplicate launch is harmless only when the existing owner is
            // our same-user executable. A spoofed/colliding pipe fails closed.
            if let Ok(Ok(reply)) = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                ipc::control(&name, serde_json::json!({"op":"snapshot"})),
            )
            .await
                && reply["ok"] == true
            {
                return Ok(());
            }
            return Err(error.into());
        }
    };
    let path = dir.join("profiles.json");
    let mut snapshot = Snapshot {
        startup: startup::enabled(),
        data_directory: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut readonly = false;
    match config::load(&path) {
        Ok(profiles) => {
            snapshot.profiles = profiles;
        }
        Err(e) => {
            snapshot.error = Some(e);
            readonly = true;
        }
    }
    for p in &snapshot.profiles {
        let mut c = ConnectionState::default();
        c.set("stopped", "Ready", None);
        snapshot.connections.insert(p.id.clone(), c);
    }
    if path.exists() && !dir.join("profiles.pre-agent.json").exists() {
        std::fs::copy(&path, dir.join("profiles.pre-agent.json"))?;
    }
    let store = state::Store::new(snapshot);
    let journal = journal::Journal::start(dir.clone()).await;
    let _ = store.journal.set(journal.clone());
    let shutdown = CancellationToken::new();
    let (recovery_tx, recovery_rx) = watch::channel(ssh::Recovery::Ready);
    platform::start(
        recovery_tx,
        shutdown.clone(),
        !args.iter().any(|s| s == "--no-tray"),
    )?;
    let (commands, rx) = mpsc::channel(32);
    let state = store.clone();
    let cancel = shutdown.clone();
    let server =
        tokio::spawn(
            async move { ipc::serve(listener, name, security, state, commands, cancel).await },
        );
    let cancel = shutdown.clone();
    let state = store.clone();
    let notifier = tokio::spawn(async move {
        let mut changes = state.changes.subscribe();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = changes.changed() => platform::update_tray(&state.read())
            }
        }
    });
    let manager = manager::Manager::new(
        store,
        path,
        recovery_rx,
        readonly,
        args.iter().any(|s| s == "--test-mode"),
    );
    let mut management = tokio::spawn(manager.run(rx, shutdown.clone()));
    tokio::select! {
        result = server => {
            shutdown.cancel();
            let _ = management.await;
            result??;
        },
        result = &mut management => {
            shutdown.cancel();
            result?;
        }
    }
    notifier.abort();
    // Drain queued records after graceful tunnel cleanup, before the runtime exits.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        journal.request(serde_json::json!({"op":"log_options"})),
    )
    .await;
    platform::stop();
    Ok(())
}
