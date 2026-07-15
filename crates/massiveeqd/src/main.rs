mod api;
mod device;
mod filter_host;

use anyhow::Result;
use api::Service;
use filter_host::FilterHost;
use massiveeq_core::{DeviceInfo, Library, Storage};
use std::sync::Arc;
use tokio::{
    signal,
    sync::Mutex,
    time::{Duration, interval},
};

pub struct AppState {
    storage: Storage,
    library: Library,
    devices: Vec<DeviceInfo>,
    host: FilterHost,
}

pub async fn refresh_locked(state: &mut AppState, force_restart: bool) -> Result<()> {
    state.devices = device::discover(&state.library).await.unwrap_or_default();
    state
        .host
        .reconcile(
            &state.storage,
            &state.library,
            &state.devices,
            force_restart,
        )
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("massiveeqd {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arguments.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "massiveeqd {}\n\nMassiveEQ user-session audio service",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if let Some(argument) = arguments.first() {
        anyhow::bail!("unknown option: {argument}");
    }

    let storage = Storage::discover()?;
    let mut library = storage.load_library()?;
    if library.profiles.is_empty() {
        let _ = storage.create_profile(&mut library, "Flat")?;
    }
    let host = FilterHost::new().await?;
    let state = Arc::new(Mutex::new(AppState {
        storage,
        library,
        devices: Vec::new(),
        host,
    }));
    // Own the control name before creating any PipeWire filters. A second
    // daemon must fail without touching the live audio graph.
    let connection = zbus::connection::Builder::session()?
        .name("org.massiveeq.Service1")?
        .serve_at(
            "/org/massiveeq/Service1",
            Service {
                state: state.clone(),
            },
        )?
        .build()
        .await?;

    {
        let mut guard = state.lock().await;
        refresh_locked(&mut guard, false).await?;
    }

    let poll_state = state.clone();
    let poller = tokio::spawn(async move {
        let mut timer = interval(Duration::from_secs(2));
        loop {
            timer.tick().await;
            let mut guard = poll_state.lock().await;
            let _ = refresh_locked(&mut guard, false).await;
        }
    });

    shutdown_signal().await?;
    poller.abort();
    state.lock().await.host.stop_all().await;
    drop(connection);
    Ok(())
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c().await?;
        Ok(())
    }
}
