use crate::volume_guard::VolumeGuard;
use anyhow::{Context, Result};
use pipewire as pw;
use pw::{
    proxy::ProxyT,
    spa::{
        self,
        pod::{Pod, Value, ValueArray, deserialize::PodDeserializer},
    },
    types::ObjectType,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
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

type MonitoredObjects = Rc<RefCell<Vec<(u32, Box<dyn ProxyT>, Box<dyn pw::proxy::Listener>)>>>;

#[derive(Default)]
struct RegistryChanges {
    relevant_ids: HashSet<u32>,
}

impl RegistryChanges {
    fn added(&mut self, id: u32, kind: &ObjectType) -> bool {
        // Discovery uses short-lived pw-dump/pw-metadata clients. Treating
        // their connection events as device changes triggers discovery again.
        if !matches!(
            kind,
            ObjectType::Device | ObjectType::Node | ObjectType::Metadata
        ) {
            return false;
        }
        self.relevant_ids.insert(id);
        true
    }

    fn removed(&mut self, id: u32) -> bool {
        self.relevant_ids.remove(&id)
    }
}

pub struct DeviceMonitor {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

/// A volume to put back on a node, by name, after a blocked jump.
pub type VolumeRevert = (String, f64);

impl DeviceMonitor {
    pub fn spawn(
        events: Sender<()>,
        guard: Arc<VolumeGuard>,
        reverts: Sender<VolumeRevert>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("massiveeq-registry".into())
            .spawn(move || run_monitor(events, guard, reverts, thread_stop, started_tx))
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
    guard: Arc<VolumeGuard>,
    reverts: Sender<VolumeRevert>,
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
        let removal_store = objects.clone();
        let global_events = events.clone();
        let changes = Rc::new(RefCell::new(RegistryChanges::default()));
        let names: Rc<RefCell<HashMap<u32, String>>> = Rc::default();
        let removed_names = names.clone();
        let added_changes = changes.clone();
        let removed_changes = changes;
        let _listener = registry
            .add_listener_local()
            .global(move |object| {
                if !added_changes.borrow_mut().added(object.id, &object.type_) {
                    return;
                }
                let _ = global_events.try_send(());
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                match object.type_ {
                    ObjectType::Node => {
                        if let Ok(node) = registry.bind::<pw::node::Node, _>(object) {
                            let update_events = global_events.clone();
                            let id = object.id;
                            let info_names = names.clone();
                            let param_names = names.clone();
                            let guard = guard.clone();
                            let reverts = reverts.clone();
                            let listener = node
                                .add_listener_local()
                                .info(move |info| {
                                    if let Some(name) =
                                        info.props().and_then(|props| props.get("node.name"))
                                    {
                                        info_names.borrow_mut().insert(id, name.to_owned());
                                    }
                                    let _ = update_events.try_send(());
                                })
                                // Volume changes, as they happen: an app or a
                                // Bluetooth device raising a guarded output's
                                // volume is undone at once.
                                .param(move |_, kind, _, _, param| {
                                    if kind != spa::param::ParamType::Props {
                                        return;
                                    }
                                    let Some(level) = param.and_then(props_volume_level) else {
                                        return;
                                    };
                                    let Some(name) = param_names.borrow().get(&id).cloned() else {
                                        return;
                                    };
                                    if let Some(restore) = guard.observe(&name, level) {
                                        let _ = reverts.try_send((name, restore));
                                    }
                                })
                                .register();
                            node.subscribe_params(&[spa::param::ParamType::Props]);
                            object_store.borrow_mut().push((
                                object.id,
                                Box::new(node),
                                Box::new(listener),
                            ));
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
                            object_store.borrow_mut().push((
                                object.id,
                                Box::new(metadata),
                                Box::new(listener),
                            ));
                        }
                    }
                    _ => {}
                }
            })
            .global_remove(move |id| {
                removed_names.borrow_mut().remove(&id);
                let removed = {
                    let mut objects = removal_store.borrow_mut();
                    objects
                        .iter()
                        .position(|(object_id, _, _)| *object_id == id)
                        .map(|index| objects.swap_remove(index))
                };
                // Listener destruction may interact with PipeWire. Do it
                // after releasing the RefCell borrow to avoid re-entrancy.
                drop(removed);
                if removed_changes.borrow_mut().removed(id) {
                    let _ = events.try_send(());
                }
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

/// The desktop volume level of a node's Props: its loudest channel.
fn props_volume_level(param: &Pod) -> Option<f64> {
    let (_, value) = PodDeserializer::deserialize_any_from(param.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    object
        .properties
        .into_iter()
        .find_map(|property| match (property.key, property.value) {
            (spa::sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(volumes))) => {
                volumes
                    .iter()
                    .map(|volume| massiveeq_core::volume_level(*volume as f64))
                    .reduce(f64::max)
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_loudest_channel_volume_from_props() {
        let props = Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamProps.as_raw(),
            id: spa::param::ParamType::Props.as_raw(),
            properties: vec![
                spa::pod::Property::new(spa::sys::SPA_PROP_mute, Value::Bool(false)),
                spa::pod::Property::new(
                    spa::sys::SPA_PROP_channelVolumes,
                    Value::ValueArray(ValueArray::Float(vec![0.125, 0.064])),
                ),
            ],
        });
        let bytes =
            spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &props)
                .unwrap()
                .0
                .into_inner();
        let level = props_volume_level(Pod::from_bytes(&bytes).unwrap()).unwrap();
        assert!((level - 0.5).abs() < 1e-6);
    }

    #[test]
    fn observer_clients_do_not_trigger_discovery_on_connect_or_disconnect() {
        let mut changes = RegistryChanges::default();
        assert!(!changes.added(10, &ObjectType::Client));
        assert!(!changes.removed(10));
    }

    #[test]
    fn ports_do_not_trigger_device_discovery() {
        let mut changes = RegistryChanges::default();
        assert!(!changes.added(11, &ObjectType::Port));
        assert!(!changes.removed(11));
    }

    #[test]
    fn device_node_and_metadata_changes_still_trigger_discovery() {
        let mut changes = RegistryChanges::default();
        for (id, kind) in [
            (12, ObjectType::Device),
            (13, ObjectType::Node),
            (14, ObjectType::Metadata),
        ] {
            assert!(changes.added(id, &kind));
            assert!(changes.removed(id));
        }
    }

    #[test]
    fn unknown_or_already_removed_globals_do_not_trigger_discovery() {
        let mut changes = RegistryChanges::default();
        assert!(!changes.removed(999));
        assert!(changes.added(15, &ObjectType::Node));
        assert!(changes.removed(15));
        assert!(!changes.removed(15));
    }
}
