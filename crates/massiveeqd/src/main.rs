mod api;
mod audio_engine;
mod device;
mod device_monitor;
mod filter_host;
mod native_filter;

use anyhow::Result;
use api::Service;
use filter_host::FilterHost;
use massiveeq_core::{DeviceInfo, DeviceKey, Library, Storage};
use massiveeq_dsp::{ProfileProcessor, compile_bypass};
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
    let devices = device::discover(&state.library).await?;
    state
        .host
        .reconcile(&state.storage, &state.library, &devices, force_restart)
        .await?;
    state.devices = devices;
    Ok(())
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
            "massiveeqd {}\n\nMassiveEQ user-session audio service\n\nDiagnostics:\n  --self-test-node NODE_NAME",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|value| value == "--self-test-node")
    {
        let node_name = arguments
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("--self-test-node requires a PipeWire node name"))?;
        let seconds = arguments
            .get(2)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(2)
            .clamp(1, 30);
        return native_self_test(node_name, seconds).await;
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

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1);
    let monitor = device_monitor::DeviceMonitor::spawn(event_tx.clone())?;
    let event_state = state.clone();
    let event_task = tokio::spawn(async move {
        while event_rx.recv().await.is_some() {
            // Bluetooth and route changes arrive as bursts. Coalesce them so
            // the active chain is reconciled once from the final registry view.
            tokio::time::sleep(Duration::from_millis(75)).await;
            while event_rx.try_recv().is_ok() {}
            let mut guard = event_state.lock().await;
            let _ = refresh_locked(&mut guard, false).await;
        }
    });
    // A slow health pulse catches a terminated native filter even if the
    // registry itself is otherwise idle; discovery remains event-driven.
    let health_task = tokio::spawn(async move {
        let mut timer = interval(Duration::from_secs(30));
        loop {
            timer.tick().await;
            if event_tx.send(()).await.is_err() {
                break;
            }
        }
    });

    shutdown_signal().await?;
    event_task.abort();
    health_task.abort();
    monitor.stop();
    state.lock().await.host.stop_all().await;
    drop(connection);
    Ok(())
}

async fn native_self_test(node_name: &str, seconds: u64) -> Result<()> {
    let settings = device::graph_settings().await;
    let compiled = compile_bypass(settings.sample_rate, settings.quantum, 2);
    let processor = Box::new(ProfileProcessor::new(&compiled)?);
    let filter = native_filter::NativeFilter::spawn(
        DeviceInfo {
            key: DeviceKey {
                backend: "test".into(),
                stable_id: node_name.into(),
                route: "self-test".into(),
            },
            node_name: node_name.into(),
            description: "Native pipeline self-test".into(),
            channels: 2,
            connected: true,
            assigned_profile: None,
            bypassed: false,
        },
        processor,
        settings.sample_rate,
        settings.quantum,
        compiled.latency_frames,
    )?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let stats = serde_json::json!({
        "sample_rate": settings.sample_rate,
        "quantum_capacity": settings.quantum,
        "node": filter.node_name,
        "input_overflows": filter.stats.input_overflows.load(std::sync::atomic::Ordering::Relaxed),
        "output_underflows": filter.stats.output_underflows.load(std::sync::atomic::Ordering::Relaxed),
        "invalid_buffers": filter.stats.invalid_buffers.load(std::sync::atomic::Ordering::Relaxed),
        "process_calls": filter.stats.process_calls.load(std::sync::atomic::Ordering::Relaxed),
        "process_peak_us": filter.stats.process_nanos_peak.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0,
    });
    filter.stop();
    println!("{stats}");
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

#[cfg(test)]
mod allocation_test_support {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
    };

    pub struct TrackingAllocator;
    thread_local! { static TRACK: Cell<bool> = const { Cell::new(false) }; }
    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            TRACK.with(|track| {
                if track.get() {
                    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                }
            });
            // SAFETY: delegated to the system allocator with the original layout.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            TRACK.with(|track| {
                if track.get() {
                    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                }
            });
            // SAFETY: pointer and layout came from the system allocator.
            unsafe { System.dealloc(pointer, layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            TRACK.with(|track| {
                if track.get() {
                    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                }
            });
            // SAFETY: pointer and layout came from the system allocator.
            unsafe { System.realloc(pointer, layout, size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator;

    pub fn begin() {
        ALLOCATIONS.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
    }

    pub fn end() -> usize {
        TRACK.with(|track| track.set(false));
        ALLOCATIONS.load(Ordering::Relaxed)
    }
}
