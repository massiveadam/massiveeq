use anyhow::{Context, Result};
use pipewire as pw;
use pw::{proxy::ProxyT, types::ObjectType};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio::sync::mpsc::Sender;

type MonitoredObjects = Rc<RefCell<Vec<(Box<dyn ProxyT>, Box<dyn pw::proxy::Listener>)>>>;

pub struct DeviceMonitor {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl DeviceMonitor {
    pub fn spawn(events: Sender<()>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("massiveeq-registry".into())
            .spawn(move || run_monitor(events, thread_stop, started_tx))
            .context("failed to start the PipeWire registry monitor")?;
        match started_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                anyhow::bail!(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                anyhow::bail!("PipeWire registry monitor did not start")
            }
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for DeviceMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn run_monitor(
    events: Sender<()>,
    stop: Arc<AtomicBool>,
    started: mpsc::SyncSender<std::result::Result<(), String>>,
) -> Result<()> {
    let result = (|| -> Result<()> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;
        let registry_weak = registry.downgrade();
        let objects: MonitoredObjects = Rc::new(RefCell::new(Vec::new()));
        let object_store = objects.clone();
        let global_events = events.clone();
        let _listener = registry
            .add_listener_local()
            .global(move |object| {
                let _ = global_events.try_send(());
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                match object.type_ {
                    ObjectType::Node => {
                        if let Ok(node) = registry.bind::<pw::node::Node, _>(object) {
                            let update_events = global_events.clone();
                            let listener = node
                                .add_listener_local()
                                .info(move |_| {
                                    let _ = update_events.try_send(());
                                })
                                .register();
                            object_store
                                .borrow_mut()
                                .push((Box::new(node), Box::new(listener)));
                        }
                    }
                    ObjectType::Metadata => {
                        if let Ok(metadata) = registry.bind::<pw::metadata::Metadata, _>(object) {
                            let update_events = global_events.clone();
                            let listener = metadata
                                .add_listener_local()
                                .property(move |_, _, _, _| {
                                    let _ = update_events.try_send(());
                                    0
                                })
                                .register();
                            object_store
                                .borrow_mut()
                                .push((Box::new(metadata), Box::new(listener)));
                        }
                    }
                    _ => {}
                }
            })
            .global_remove(move |_| {
                let _ = events.try_send(());
            })
            .register();
        let weak = mainloop.downgrade();
        let timer = mainloop.loop_().add_timer(move |_| {
            if stop.load(Ordering::Acquire)
                && let Some(mainloop) = weak.upgrade()
            {
                mainloop.quit();
            }
        });
        timer
            .update_timer(
                Some(Duration::from_millis(100)),
                Some(Duration::from_millis(100)),
            )
            .into_result()?;
        let _ = started.send(Ok(()));
        mainloop.run();
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = started.send(Err(error.to_string()));
    }
    result
}
